use super::{
    fake_provider::FakeProviderConfig, recording_fixtures, relay_support::Envelope, support::App,
};
use cmaeg::{
    activity::{Change, ChannelActivity, ConsumerState},
    delivery::intents::DeliveryCommand,
    turns::{observer::ActivityObserver, outcomes::TurnOutcome},
};
use cmaeg_core::ids::{ConsumerId, EventId, RecordId};
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

#[tokio::test]
async fn terminal_only_provider_answer_records_produced_before_completion_and_delivery() {
    let lifecycle = Arc::new(Lifecycle::default());
    let mut app = App::observed_with(0, Default::default(), FakeProviderConfig {
        frames: Some(vec![
            json!({"type":"response.created","response":{"id":"terminal-response"}}),
            json!({"type":"response.completed","response":{"id":"terminal-response","output":[{"type":"message","content":[{"type":"output_text","text":"terminal answer"}]}]}}),
        ]),
        ..Default::default()
    }, lifecycle.clone()).await;
    assert_eq!(app.fixture.send(&Envelope::new()).await.status(), 202);
    assert!(
        app.tracker
            .drain(Instant::now() + Duration::from_secs(10))
            .await
    );
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
    let produced = consumer.produced_at.as_ref().unwrap();
    assert!(produced <= consumer.completed_at.as_ref().unwrap());
    assert_eq!(consumer.deliveries.len(), 1);
    assert_eq!(consumer.deliveries[0].step, "answer");
    assert_eq!(consumer.deliveries[0].outcome, "confirmed");
    assert!(produced <= consumer.deliveries[0].confirmed_at.as_ref().unwrap());
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
    let mut app = App::observed(0, lifecycle.clone()).await;
    app.outcome(accepted("response")).await;
    app.outcome(completed("answer")).await;
    app.outcome(completed("answer")).await;
    app.drain().await;
    assert_eq!(app.fake.texts(), ["answer"]);
    let row = lifecycle.row(&app);
    assert!(row.consumers[0].produced_at.is_none());
    assert_eq!(row.consumers[0].state, ConsumerState::Completed);
    assert_eq!(row.consumers[0].deliveries.len(), 1);
    assert_eq!(row.consumers[0].deliveries[0].outcome, "confirmed");
}
