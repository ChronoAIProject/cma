use super::{
    fake_provider::FakeProviderConfig,
    recording_fixtures,
    relay_support::{now, Envelope},
    support::{threaded_lark_envelope, App},
};
use cmaeg::{
    activity::{Change, ChannelActivity, ConsumerState},
    delivery::intents::DeliveryCommand,
    turns::{observer::ActivityObserver, outcomes::TurnOutcome},
};
use cmaeg_core::ids::{ConsumerId, EventId, MessageRef, RecordId};
use cmaeg_core::seal::{ReplyAddress, ReplyTargetRef};
use futures_util::future::BoxFuture;
use serde_json::json;
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Default)]
struct Lifecycle {
    changes: Mutex<Vec<(RecordId, EventId, Change)>>,
    discard_produced: bool,
}
impl ActivityObserver for Lifecycle {
    fn checkpoint<'a>(
        &'a self,
        record: &'a RecordId,
        event: &'a EventId,
        change: Change,
    ) -> BoxFuture<'a, ()> {
        if !(self.discard_produced && change.state == Some(ConsumerState::Produced)) {
            self.changes
                .lock()
                .unwrap()
                .push((record.clone(), event.clone(), change));
        }
        Box::pin(async {})
    }
}
impl Lifecycle {
    fn row(&self, app: &App) -> ChannelActivity {
        let activity = &app.turn.activity;
        let mut row = serde_json::from_value(json!({
            "event_id": activity.event_id, "source": activity.source,
            "actor": activity.actor, "conversation": activity.conversation,
            "occurred_at": activity.occurred_at, "received_at": activity.occurred_at,
            "consumers": []
        }))
        .unwrap();
        for (record, event, change) in self.changes.lock().unwrap().iter() {
            assert_eq!(record, &app.event.record_id);
            assert_eq!(event, &app.event.event_id);
            change.apply(&mut row);
        }
        row
    }
    fn states(&self) -> Vec<ConsumerState> {
        self.changes
            .lock()
            .unwrap()
            .iter()
            .filter_map(|(_, _, change)| change.state.clone())
            .collect()
    }
}
fn accepted(id: &str) -> TurnOutcome {
    TurnOutcome::Accepted {
        response_id: id.into(),
        thread_reply: false,
    }
}
fn completed(text: &str) -> TurnOutcome {
    TurnOutcome::Completed {
        final_text: Some(text.into()),
    }
}

fn timestamp(value: &str) -> time::OffsetDateTime {
    time::OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339).unwrap()
}

async fn terminal_answer(
    lifecycle: Arc<Lifecycle>,
    reported_response: &str,
    preceding_frames: Vec<serde_json::Value>,
) -> App {
    let mut frames = vec![json!({"type":"response.created","response":{"id":"terminal-response"}})];
    frames.extend(preceding_frames);
    frames.push(json!({"type":"response.completed","response":{"id":reported_response,"output":[{"type":"message","content":[{"type":"output_text","text":"terminal answer"}]}]}}));
    let app = App::observed_with(
        0,
        Default::default(),
        FakeProviderConfig {
            frames: Some(frames),
            ..Default::default()
        },
        lifecycle,
    )
    .await;
    assert_eq!(app.fixture.send(&Envelope::new()).await.status(), 202);
    assert!(
        app.tracker
            .drain(Instant::now() + Duration::from_secs(10))
            .await
    );
    app
}

#[tokio::test]
async fn terminal_only_provider_answer_records_produced_before_completion_and_delivery() {
    let lifecycle = Arc::new(Lifecycle::default());
    let mut app = terminal_answer(lifecycle.clone(), "terminal-response", vec![]).await;
    app.drain().await;
    assert_eq!(app.fake.texts(), ["terminal answer"]);
    assert_eq!(
        lifecycle.states(),
        [
            ConsumerState::Accepted,
            ConsumerState::Produced,
            ConsumerState::Completed
        ]
    );
    let row = lifecycle.row(&app);
    let consumer = &row.consumers[0];
    assert_eq!(consumer.consumer_id, "default");
    assert_eq!(consumer.response_id.as_deref(), Some("terminal-response"));
    assert_eq!(consumer.state, ConsumerState::Completed);
    let produced = timestamp(consumer.produced_at.as_deref().unwrap());
    assert!(produced <= timestamp(consumer.completed_at.as_deref().unwrap()));
    assert_eq!(consumer.deliveries.len(), 1);
    assert_eq!(consumer.deliveries[0].step, "answer");
    assert_eq!(consumer.deliveries[0].outcome, "confirmed");
    assert_eq!(
        consumer.deliveries[0].answer_response_id.as_deref(),
        Some("terminal-response")
    );
    assert!(produced <= timestamp(consumer.deliveries[0].confirmed_at.as_deref().unwrap()));
    let changes = lifecycle.changes.lock().unwrap();
    let produced_index = changes
        .iter()
        .position(|(_, _, c)| c.state == Some(ConsumerState::Produced))
        .unwrap();
    let delivered_index = changes
        .iter()
        .position(|(_, _, c)| c.delivery.is_some())
        .unwrap();
    assert!(produced_index < delivered_index);
}

#[tokio::test]
async fn repeated_terminal_and_committed_answers_keep_the_first_produced_time_and_one_send() {
    for committed in [false, true] {
        let lifecycle = Arc::new(Lifecycle::default());
        let mut app = App::observed(0, lifecycle.clone()).await;
        app.outcome(accepted("response")).await;
        if committed {
            app.outcome(TurnOutcome::MessageCommitted {
                text: "answer".into(),
            })
            .await;
        }
        app.outcome(completed("answer")).await;
        app.outcome(completed("answer")).await;
        app.drain().await;
        assert_eq!(app.fake.texts(), ["answer"]);
        let first = lifecycle
            .changes
            .lock()
            .unwrap()
            .iter()
            .find(|(_, _, c)| c.state == Some(ConsumerState::Produced))
            .unwrap()
            .2
            .at
            .clone();
        let mut row = lifecycle.row(&app);
        assert_eq!(
            row.consumers[0].produced_at.as_deref(),
            Some(first.as_str())
        );
        for (_, _, change) in lifecycle.changes.lock().unwrap().iter() {
            let mut replay = change.clone();
            replay.at = "2099-01-01T00:00:00Z".into();
            replay.apply(&mut row);
        }
        assert_eq!(
            row.consumers[0].produced_at.as_deref(),
            Some(first.as_str())
        );
        assert_eq!(row.consumers[0].state, ConsumerState::Completed);
        assert_eq!(row.consumers[0].response_id.as_deref(), Some("response"));
    }
}

#[tokio::test]
async fn empty_terminal_and_failure_notices_do_not_invent_produced() {
    for outcome in [
        TurnOutcome::Completed { final_text: None },
        completed(""),
        completed(" \n\t"),
        TurnOutcome::Failed {
            code: "failed".into(),
            message: "diagnostic".into(),
        },
        TurnOutcome::Incomplete {
            reason: "limit".into(),
        },
    ] {
        let lifecycle = Arc::new(Lifecycle::default());
        let mut app = App::observed(0, lifecycle.clone()).await;
        app.outcome(accepted("response")).await;
        app.outcome(outcome).await;
        app.drain().await;
        assert!(!lifecycle.states().contains(&ConsumerState::Produced));
        assert!(lifecycle.row(&app).consumers[0].produced_at.is_none());
    }
}

#[tokio::test]
async fn progress_and_tool_reply_do_not_invent_produced_or_duplicate_the_final_answer() {
    let lifecycle = Arc::new(Lifecycle::default());
    let mut app = App::observed_with(
        1,
        cmaeg::DeliveryConfig {
            heartbeat: Duration::from_millis(100),
            ..Default::default()
        },
        FakeProviderConfig::default(),
        lifecycle.clone(),
    )
    .await;
    app.outcome(accepted("response")).await;
    app.fake.wait(2).await;
    assert_eq!(app.fake.texts()[0], "Working on it…");
    assert!(app.fake.texts().iter().any(|text| text == "Still working…"));
    assert_eq!(
        app.reply(json!({"text":"tool answer"}))
            .await
            .delivery
            .kind(),
        "confirmed"
    );
    assert!(lifecycle.row(&app).consumers[0].produced_at.is_none());
    app.outcome(completed("final answer")).await;
    app.drain().await;
    let texts = app.fake.texts();
    assert_eq!(
        texts.iter().filter(|text| *text == "tool answer").count(),
        1
    );
    assert!(texts.iter().all(|text| matches!(
        text.as_str(),
        "Working on it…" | "Still working…" | "tool answer"
    )));
    assert!(lifecycle.row(&app).consumers[0].produced_at.is_some());
}

#[tokio::test]
async fn live_targeted_reply_preserves_event_receipt_and_suppresses_automatic_final() {
    for targeted in [false, true] {
        let lifecycle = Arc::new(Lifecycle::default());
        let terminal = Arc::new(tokio::sync::Semaphore::new(0));
        let mut app = App::observed_threaded_lark(
            FakeProviderConfig {
                terminal_gate: Some(terminal.clone()),
                frames: Some(vec![
                    json!({"type":"response.created","response":{"id":"resp_live_answer","thread_reply":true}}),
                    json!({"type":"response.completed","response":{"id":"resp_live_answer","output":[{"type":"message","id":"item_live_answer","content":[{"type":"output_text","text":"automatic answer"}]}]}}),
                ]),
                ..Default::default()
            },
            lifecycle.clone(),
        )
        .await;
        assert_eq!(
            app.fixture.send(&threaded_lark_envelope()).await.status(),
            202
        );
        app.fake.wait(1).await;
        assert_eq!(app.fake.texts(), ["Working on it…"]);

        let row = lifecycle.row(&app);
        assert_eq!(
            row.consumers[0].response_id.as_deref(),
            Some("resp_live_answer")
        );
        assert!(row.consumers[0].produced_at.is_none());
        let answer = if targeted {
            let client = &app.fixture.client;
            let context = app
                .provider
                .admitted()
                .into_iter()
                .find(|request| request.path == "/api/v2/responses")
                .unwrap();
            let body: serde_json::Value = serde_json::from_str(&context.body).unwrap();
            let event = body["event_context"]["event_ref"].as_str().unwrap();
            let target = client
                .post(format!("{}/v1/events/{event}/reply-target", app.base))
                .bearer_auth(&app.key)
                .json(&json!({}))
                .send()
                .await
                .unwrap();
            let status = target.status();
            let target: serde_json::Value = target.json().await.unwrap();
            assert_eq!(status, 200, "{target}");
            let target: ReplyTargetRef = target["reply_target_ref"]
                .as_str()
                .unwrap()
                .parse()
                .unwrap();
            let sealed = target
                .open(&app.fixture.state.master_keys, now())
                .unwrap()
                .payload;
            assert_eq!(sealed.event.event_id, app.event.event_id);
            assert_eq!(sealed.event.channel_id, app.event.channel_id);
            assert_eq!(
                sealed.event.consumer.as_ref().filter(|id| !id.is_default()),
                app.event.consumer.as_ref()
            );
            assert!(matches!(
                sealed.event.reply,
                ReplyAddress::NyxidRelay { thread_reply: true, ref route_id, ref message_id, .. }
                    if route_id == "route-1" && message_id == "message-1"
            ));
            let response = client
                .post(format!(
                    "{}/v1/reply-targets/{}/messages",
                    app.base,
                    target.as_str()
                ))
                .bearer_auth(&app.key)
                .header("idempotency-key", "live-answer")
                .json(&json!({"text":"targeted answer","thread_reply":true}))
                .send()
                .await
                .unwrap();
            let status = response.status();
            let receipt: serde_json::Value = response.json().await.unwrap();
            assert_eq!(status, 200, "{receipt}");
            assert_eq!(receipt["delivery"]["outcome"], "confirmed");
            assert_eq!(receipt["delivery"]["platform_message_id"], "platform-2");
            let message: MessageRef = receipt["message_ref"].as_str().unwrap().parse().unwrap();
            let reference = message
                .open(&app.fixture.state.master_keys)
                .unwrap()
                .payload;
            assert_eq!(reference.event_id, app.event.event_id);
            assert_eq!(reference.channel_id, app.event.channel_id);
            assert_eq!(reference.consumer, sealed.event.consumer);
            assert_eq!(reference.platform_message_id, "platform-2");
            assert_eq!(reference.route_id, "route-1");
            "targeted answer"
        } else {
            "automatic answer"
        };

        terminal.add_permits(1);
        assert!(
            app.tracker
                .drain(Instant::now() + Duration::from_secs(10))
                .await
        );
        app.drain().await;
        assert_eq!(app.fake.texts(), ["Working on it…", answer]);
        let requests = app.fake.requests.lock().unwrap();
        assert!(requests
            .iter()
            .all(|request| request.body["message_id"] == "message-1"
                && request.body["thread_reply"] == true));
        let row = lifecycle.row(&app);
        let consumer = &row.consumers[0];
        assert_eq!(consumer.state, ConsumerState::Completed);
        assert_eq!(consumer.response_id.as_deref(), Some("resp_live_answer"));
        assert_eq!(consumer.deliveries.len(), 2);
        let ack = &consumer.deliveries[0];
        assert_eq!(ack.step, "ack");
        assert_eq!(ack.outcome, "confirmed");
        assert!(ack.answer_response_id.is_none());
        let answer = &consumer.deliveries[1];
        assert_eq!(answer.destination, "origin");
        assert_eq!(answer.outcome, "confirmed");
        assert_eq!(answer.platform_message_id.as_deref(), Some("platform-2"));
        assert_eq!(answer.step, if targeted { "tool_reply" } else { "answer" });
        assert_eq!(
            answer.answer_response_id.as_deref(),
            (!targeted).then_some("resp_live_answer")
        );
    }
}

#[tokio::test]
async fn terminal_production_stays_with_its_consumer_and_response() {
    let lifecycle = Arc::new(Lifecycle::default());
    let mut app = App::observed(0, lifecycle.clone()).await;
    for (id, response, outcome) in [
        ("csm_00000000000a", "response-a", completed("A answer")),
        (
            "csm_00000000000b",
            "response-b",
            TurnOutcome::Completed { final_text: None },
        ),
    ] {
        let mut event = app.event.clone();
        event.consumer = Some(ConsumerId::parse(id).unwrap());
        let mut turn = recording_fixtures::turn(
            &app.fixture.state.master_keys,
            &app.turn.channel,
            app.turn.activity.clone(),
            &event,
        );
        Arc::get_mut(&mut turn).unwrap().consumer = event.consumer;
        for outcome in [accepted(response), outcome] {
            app.commands
                .send(DeliveryCommand::StreamOutcome {
                    turn: turn.clone(),
                    outcome,
                    answer_response_id: Some(response.into()),
                    accepted_deadline: None,
                    recovery: None,
                })
                .await
                .unwrap();
        }
    }
    app.drain().await;
    assert_eq!(app.fake.texts(), ["A answer"]);
    let row = lifecycle.row(&app);
    assert_eq!(row.consumers.len(), 2);
    let a = row
        .consumers
        .iter()
        .find(|c| c.consumer_id == "csm_00000000000a")
        .unwrap();
    let b = row
        .consumers
        .iter()
        .find(|c| c.consumer_id == "csm_00000000000b")
        .unwrap();
    assert_eq!(a.response_id.as_deref(), Some("response-a"));
    assert_eq!(b.response_id.as_deref(), Some("response-b"));
    assert!(a.produced_at.is_some());
    assert!(b.produced_at.is_none());
    assert_eq!(a.deliveries.len(), 1);
    assert_eq!(
        a.deliveries[0].answer_response_id.as_deref(),
        Some("response-a")
    );
    assert!(b.deliveries.is_empty());
    assert_eq!(a.state, ConsumerState::Completed);
    assert_eq!(b.state, ConsumerState::Completed);
}

#[tokio::test]
async fn lost_produced_checkpoint_never_retries_the_answer() {
    let lifecycle = Arc::new(Lifecycle {
        discard_produced: true,
        ..Default::default()
    });
    let mut app = terminal_answer(lifecycle.clone(), "terminal-response", vec![]).await;
    app.outcome(completed("terminal answer")).await;
    app.drain().await;
    assert_eq!(app.fake.texts(), ["terminal answer"]);
    let row = lifecycle.row(&app);
    assert!(row.consumers[0].produced_at.is_none());
    assert_eq!(row.consumers[0].state, ConsumerState::Completed);
    assert_eq!(row.consumers[0].deliveries.len(), 1);
    assert_eq!(row.consumers[0].deliveries[0].outcome, "confirmed");
    assert_eq!(
        row.consumers[0].deliveries[0].answer_response_id.as_deref(),
        Some("terminal-response")
    );
}

#[tokio::test]
async fn conflicting_provider_identity_does_not_bind_a_confirmed_answer() {
    let lifecycle = Arc::new(Lifecycle {
        discard_produced: true,
        ..Default::default()
    });
    let mut app = terminal_answer(lifecycle.clone(), "different-response", vec![]).await;
    app.drain().await;
    assert_eq!(app.fake.texts(), ["terminal answer"]);
    let row = lifecycle.row(&app);
    let consumer = &row.consumers[0];
    assert_eq!(consumer.response_id.as_deref(), Some("terminal-response"));
    assert_eq!(consumer.deliveries[0].outcome, "confirmed");
    assert!(consumer.deliveries[0].answer_response_id.is_none());
}

#[tokio::test]
async fn conflicting_wire_output_ids_withhold_answer_proof_without_changing_the_send() {
    for (frame, expected) in [
        (
            json!({"type":"response.output_text.done","item_id":"answer-item","response_id":"terminal-response","text":"terminal answer"}),
            Some("terminal-response"),
        ),
        (
            json!({"type":"response.output_text.done","item_id":"answer-item","response_id":"different-response","text":"terminal answer"}),
            None,
        ),
        (
            json!({"type":"response.output_item.done","output_index":0,"response":{"id":"different-response"},"item":{"type":"message","id":"answer-item","content":[{"type":"output_text","text":"terminal answer"}]}}),
            None,
        ),
        (
            json!({"type":"response.output_text.delta","item_id":"answer-item","response_id":"different-response","delta":"terminal answer"}),
            None,
        ),
    ] {
        let lifecycle = Arc::new(Lifecycle {
            discard_produced: true,
            ..Default::default()
        });
        let mut app =
            terminal_answer(lifecycle.clone(), "terminal-response", vec![frame.clone()]).await;
        app.drain().await;
        assert_eq!(app.fake.texts(), ["terminal answer"], "{}", frame["type"]);
        let row = lifecycle.row(&app);
        let consumer = &row.consumers[0];
        assert!(consumer.produced_at.is_none());
        assert_eq!(consumer.deliveries.len(), 1);
        assert_eq!(consumer.deliveries[0].outcome, "confirmed");
        assert_eq!(
            consumer.deliveries[0].answer_response_id.as_deref(),
            expected,
            "{}",
            frame["type"]
        );
    }
}
