use super::support::App;
use cmaeg::{delivery::intents::DeliveryOutcome, turns::outcomes::TurnOutcome, DeliveryConfig};
use cmaeg_core::seal::ReplyAddress;
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn tool_receipts_seal_platform_identity_consume_token_once_and_suppress_fallback() {
    let mut app = App::standard(0).await;
    app.outcome(TurnOutcome::Accepted {
        thread_reply: false,
        response_id: "response".into(),
    })
    .await;
    let first = app.reply(json!({"text":"first"})).await;
    assert_eq!(
        first.delivery,
        DeliveryOutcome::Confirmed {
            platform_message_id: Some("platform-1".into())
        }
    );
    let payload = first
        .message_ref
        .unwrap()
        .open(&app.fixture.state.master_keys)
        .unwrap()
        .payload;
    assert_eq!(payload.platform_message_id, "platform-1");
    assert_eq!(payload.event_id, app.event.event_id);
    assert_eq!(payload.route_id, "route-1");
    app.reply(json!({"text":"second"})).await;
    app.outcome(TurnOutcome::Completed {
        final_text: Some("do not duplicate".into()),
    })
    .await;
    app.drain().await;
    let records = app.fake.requests.lock().unwrap();
    assert_eq!(records.len(), 2);
    let ReplyAddress::NyxidRelay { reply_token, .. } = &app.event.reply else {
        panic!()
    };
    assert_eq!(
        records[0].authorization,
        format!("Bearer {}", reply_token.as_str())
    );
    assert_eq!(records[1].authorization, "Bearer test-route-credential");
    assert!(records[1].at.duration_since(records[0].at) >= Duration::from_secs(3));
}
#[tokio::test]
async fn committed_messages_cancel_ack_and_each_send_but_terminal_does_not_duplicate() {
    let mut app = App::standard(100).await;
    app.outcome(TurnOutcome::Accepted {
        thread_reply: false,
        response_id: "response".into(),
    })
    .await;
    app.outcome(TurnOutcome::MessageCommitted { text: "one".into() })
        .await;
    app.outcome(TurnOutcome::MessageCommitted { text: "two".into() })
        .await;
    app.outcome(TurnOutcome::Completed {
        final_text: Some("one\ntwo".into()),
    })
    .await;
    app.drain().await;
    assert_eq!(app.fake.texts(), ["one", "two"]);
}
#[tokio::test]
async fn ack_does_not_suppress_final_and_deltas_are_not_delivered() {
    let mut app = App::standard(50).await;
    app.outcome(TurnOutcome::Accepted {
        thread_reply: false,
        response_id: "response".into(),
    })
    .await;
    app.outcome(TurnOutcome::TextDelta {
        text: "partial".into(),
    })
    .await;
    app.fake.wait(1).await;
    app.outcome(TurnOutcome::Completed {
        final_text: Some("final".into()),
    })
    .await;
    app.drain().await;
    assert_eq!(app.fake.texts(), ["Working on it…", "final"]);
}
#[tokio::test]
async fn failure_is_sent_once_and_disabled_ack_stays_disabled() {
    let mut app = App::standard(0).await;
    app.outcome(TurnOutcome::Accepted {
        thread_reply: false,
        response_id: "response".into(),
    })
    .await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(app.fake.texts().is_empty());
    for _ in 0..2 {
        app.outcome(TurnOutcome::Failed {
            code: "failed".into(),
            message: "private diagnostic".into(),
        })
        .await;
    }
    app.drain().await;
    assert_eq!(app.fake.texts(), [DeliveryConfig::default().failure_text]);
}
#[tokio::test]
async fn heartbeat_is_capped_at_five_and_tool_reply_suppresses_failure() {
    let mut app = App::new(
        vec![],
        50,
        DeliveryConfig {
            heartbeat: Duration::from_millis(50),
            ..Default::default()
        },
        Default::default(),
    )
    .await;
    app.outcome(TurnOutcome::Accepted {
        thread_reply: false,
        response_id: "response".into(),
    })
    .await;
    app.fake.wait(6).await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    app.reply(json!({"text":"answer"})).await;
    app.outcome(TurnOutcome::Incomplete {
        reason: "private failure".into(),
    })
    .await;
    app.drain().await;
    assert_eq!(
        app.fake
            .texts()
            .iter()
            .filter(|text| *text == "Still working…")
            .count(),
        5
    );
    assert_eq!(app.fake.texts().len(), 7);
    assert_eq!(app.fake.texts().last().unwrap(), "answer");
}
#[tokio::test]
async fn nine_thousand_characters_send_three_ordered_parts_and_degrade_buttons() {
    let mut app = App::standard(0).await;
    let text = format!(
        "{}{}{}",
        "a".repeat(4000),
        "b".repeat(4000),
        "c".repeat(1000)
    );
    let receipt = app.reply(json!({"text":text})).await;
    assert_eq!(app.fake.texts().concat(), text);
    assert_eq!(
        app.fake.texts().iter().map(String::len).collect::<Vec<_>>(),
        [4000, 4000, 1000]
    );
    assert_eq!(
        serde_json::to_value(receipt.rendering).unwrap()["blocks"][0]["outcome"]["how"],
        "chunked:3"
    );
    assert_eq!(
        receipt.delivery,
        DeliveryOutcome::Confirmed {
            platform_message_id: Some("platform-1".into())
        }
    );
    let reply = app.fixture.client.post(format!("{}/v1/events/{}/replies", app.base, app.turn.event_ref.as_str()))
        .bearer_auth(&app.key).json(&json!({"blocks":[{"type":"buttons","id":"action","options":[{"id":"yes","label":"Yes"},{"id":"no","label":"No"}]}]}))
        .send().await.unwrap();
    assert_eq!(reply.status(), 200);
    let value: serde_json::Value = reply.json().await.unwrap();
    assert_eq!(
        value["rendering"]["blocks"][0]["outcome"]["how"],
        "numbered_text"
    );
    assert_eq!(
        app.fake.texts().last().unwrap(),
        "1) Yes\n2) No\nReply with a number."
    );
    app.drain().await;
}

#[tokio::test]
async fn thread_reply_is_explicit_on_stream_and_tools_and_bound_to_original_source() {
    let mut app = App::threaded_lark().await;
    app.outcome(TurnOutcome::Accepted {
        response_id: "thread_response".into(),
        thread_reply: true,
    })
    .await;
    app.outcome(TurnOutcome::Completed {
        final_text: Some("thread answer".into()),
    })
    .await;
    app.fake.wait(1).await;
    {
        let requests = app.fake.requests.lock().unwrap();
        assert_eq!(requests[0].body["thread_reply"], true);
        assert_eq!(requests[0].body["message_id"], "message-1");
    }
    app.reply(json!({"text":"delayed follow-up", "thread_reply":true}))
        .await;
    app.drain().await;
    assert!(app
        .fake
        .requests
        .lock()
        .unwrap()
        .iter()
        .all(|r| r.body["thread_reply"] == true && r.body["message_id"] == "message-1"));

    let mut legacy = App::on("lark", false).await;
    let refused = legacy
        .reply(json!({"text":"must not escape", "thread_reply":true}))
        .await;
    assert!(matches!(refused.delivery, DeliveryOutcome::Rejected { .. }));
    assert!(legacy.fake.requests.lock().unwrap().is_empty());
    legacy.reply(json!({"text":"legacy"})).await;
    legacy.drain().await;
    assert!(legacy.fake.requests.lock().unwrap()[0]
        .body
        .get("thread_reply")
        .is_none());
}

#[tokio::test]
async fn negotiated_metadata_does_not_change_legacy_provider_output() {
    let mut app = App::threaded_lark().await;
    app.outcome(TurnOutcome::Accepted {
        response_id: "legacy_choice".into(),
        thread_reply: false,
    })
    .await;
    app.outcome(TurnOutcome::Completed {
        final_text: Some("legacy reply".into()),
    })
    .await;
    app.drain().await;
    assert!(app.fake.requests.lock().unwrap()[0]
        .body
        .get("thread_reply")
        .is_none());
}
