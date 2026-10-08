//! Successful terminal Sync frames and real buffer-full emission failures.
use super::{
    default_fixtures::{
        completed, custom_event, default_channel, recording, run, sinks_default, telegram_default,
        telegram_messages, tool, with_sinks,
    },
    fake_http::FakeHttp,
    group_fixtures::{reviewer, routed_custom_activity},
};
use cmaeg::delivery::intents::{DeliveryCommand, DeliveryOutcome};
use cmaeg_core::{ids::ConsumerId, seal::ReplyAddress};
use serde_json::json;

#[tokio::test(start_paused = true)]
async fn a_late_consumer_reply_never_claims_sync_delivery_while_siblings_are_active() {
    use axum::response::{sse::Sse, IntoResponse};
    use cmaeg::tools::ConsumerSummary;
    use cmaeg_core::ids::ConsumerName;

    for fallback in [false, true] {
        let channel = default_channel(sinks_default(), |value| {
            if fallback {
                with_sinks(value);
            } else {
                value["routing"]
                    .as_object_mut()
                    .unwrap()
                    .remove("default_reply");
                value["reply"]["sinks"] = json!([]);
            }
        });
        let mut recording = recording(FakeHttp::platforms(), true);
        let (id, waiter) = recording
            .waiters
            .register_fanout(
                2,
                ConsumerSummary {
                    id: ConsumerId::default_consumer(),
                    name: ConsumerName::parse("lead").unwrap(),
                    label: "lead".into(),
                },
            )
            .unwrap();
        assert!(recording
            .waiters
            .set_fanout_targets(&id, &routed_custom_activity().routing.unwrap().targets,));
        let reviewer = custom_event(
            &channel,
            Some(reviewer()),
            ReplyAddress::Sync { waiter: id.clone() },
        );
        let lead = custom_event(&channel, None, ReplyAddress::Sync { waiter: id.clone() });
        run(&recording, &channel, &reviewer, [completed("Review done.")]).await;
        let receipt = tool(
            &mut recording,
            &channel,
            &reviewer,
            json!({"text":"Late answer."}),
        )
        .await;
        assert!(
            recording.waiters.registered(&id),
            "the sibling still needs its terminal"
        );
        if fallback {
            assert!(matches!(
                receipt.delivery,
                DeliveryOutcome::Confirmed { .. }
            ));
        } else {
            assert!(matches!(receipt.delivery, DeliveryOutcome::Unsupported));
        }
        run(&recording, &channel, &lead, [completed("Main done.")]).await;
        let result = recording.finish().await;
        let body =
            axum::body::to_bytes(Sse::new(waiter.stream()).into_response().into_body(), 16384)
                .await
                .unwrap();
        let wire = String::from_utf8(body.to_vec()).unwrap();
        assert!(!wire.contains("Late answer."));
        assert!(!wire.contains("cmaeg.consumer.message"));
        assert_eq!(wire.matches("event: cmaeg.consumer.completed").count(), 2);
        assert_eq!(wire.matches("event: cmaeg.fanout.completed").count(), 1);
        let replies: Vec<_> = result["calls"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|call| call["body"]["type"] == "cmaeg.response.message")
            .collect();
        assert_eq!(replies.len(), usize::from(fallback));
        if fallback {
            assert_eq!(replies[0]["body"]["data"]["destination"], "default");
            assert_eq!(replies[0]["body"]["data"]["text"], "Late answer.");
        }
    }
}

#[tokio::test(start_paused = true)]
async fn successful_sync_terminals_do_not_fall_back_after_removing_the_waiter() {
    for failed in [false, true] {
        let channel = default_channel(telegram_default(), |_| {});
        let mut recording = recording(FakeHttp::platforms(), true);
        let http = recording.http.clone();
        let waiter = recording.waiter();
        let event = custom_event(&channel, None, ReplyAddress::Sync { waiter });
        if failed {
            recording
                .submit(DeliveryCommand::ConsumerFailed {
                    channel: channel.clone(),
                    payload: event,
                    consumer: ConsumerId::default_consumer(),
                    code: "channel_capacity",
                    provider_code: None,
                })
                .await;
        } else {
            run(&recording, &channel, &event, [completed("Done.")]).await;
        }
        let result = recording.finish().await;
        assert_eq!(result["sync"].as_array().unwrap().len(), 1);
        assert_eq!(
            result["sync"][0]["event"],
            if failed {
                "cmaeg.failed"
            } else {
                "cmaeg.completed"
            }
        );
        assert!(
            http.sent().is_empty(),
            "successful origin delivery must not also reach default"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn a_full_sync_waiter_uses_default_before_attempting_any_legacy_sink() {
    for flag in [false, true] {
        for default in [false, true] {
            let channel = default_channel(telegram_default(), |value| {
                with_sinks(value);
                if !default {
                    value["routing"]
                        .as_object_mut()
                        .unwrap()
                        .remove("default_reply");
                }
            });
            let mut recording = recording(FakeHttp::platforms(), flag);
            let http = recording.http.clone();
            let (waiter, held) = recording.waiters.register().unwrap();
            for _ in 0..8 {
                assert!(recording.waiters.emit(
                    &waiter,
                    "cmaeg.text.delta",
                    &json!({"text":"queued"}),
                    false
                ));
            }
            assert!(recording.waiters.registered(&waiter));
            let event = custom_event(
                &channel,
                Some(reviewer()),
                ReplyAddress::Sync {
                    waiter: waiter.clone(),
                },
            );
            let receipt = tool(&mut recording, &channel, &event, json!({"text":"Answer."})).await;
            assert!(matches!(
                receipt.delivery,
                DeliveryOutcome::Confirmed { .. }
            ));
            assert!(
                !recording.waiters.registered(&waiter),
                "the actual failed emit removed the full waiter"
            );
            let result = recording.finish().await;
            let hooks = result["calls"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|call| call["transport"] == "webhook")
                .count();
            assert_eq!(
                telegram_messages(&http.sent()).len(),
                usize::from(flag && default)
            );
            assert_eq!(hooks, usize::from(!(flag && default)));
            for call in result["calls"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|call| call["transport"] == "webhook")
            {
                assert_eq!(call["body"]["data"]["consumer"]["id"], reviewer().as_str());
            }
            drop(held);
        }
    }
}

#[tokio::test(start_paused = true)]
async fn automatic_sync_observer_frames_remain_when_output_chooses_default() {
    let channel = default_channel(telegram_default(), |value| {
        value["routing"]["default_consumer"]["reply"] = json!({"destination":"default"});
    });
    let mut recording = recording(FakeHttp::platforms(), true);
    let http = recording.http.clone();
    let waiter = recording.waiter();
    let event = custom_event(&channel, None, ReplyAddress::Sync { waiter });
    run(&recording, &channel, &event, [completed("Done.")]).await;
    let result = recording.finish().await;
    assert_eq!(result["sync"].as_array().unwrap().len(), 1);
    assert_eq!(telegram_messages(&http.sent()).len(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_successful_sync_tool_reply_keeps_existing_sinks_without_default_delivery() {
    let channel = default_channel(telegram_default(), with_sinks);
    let mut recording = recording(FakeHttp::platforms(), true);
    let http = recording.http.clone();
    let waiter = recording.waiter();
    let event = custom_event(&channel, None, ReplyAddress::Sync { waiter });
    let receipt = tool(&mut recording, &channel, &event, json!({"text":"Tool."})).await;
    assert!(matches!(
        receipt.delivery,
        DeliveryOutcome::Confirmed { .. }
    ));
    run(&recording, &channel, &event, [completed("Done.")]).await;
    let result = recording.finish().await;
    assert!(http.sent().is_empty());
    assert_eq!(result["sync"].as_array().unwrap().len(), 2);
    let hooks: Vec<_> = result["calls"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|call| call["transport"] == "webhook")
        .collect();
    assert_eq!(hooks.len(), 2);
    assert_eq!(hooks[0]["body"]["type"], "cmaeg.response.message");
    assert!(hooks[0]["body"]["data"].get("destination").is_none());
    let identity = json!({"id":"default","name":"lead","label":"lead"});
    assert_eq!(hooks[0]["body"]["data"]["consumer"], identity);
    assert_eq!(result["sync"][0]["data"]["consumer"], identity);
}
