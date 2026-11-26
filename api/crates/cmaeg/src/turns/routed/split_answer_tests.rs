use super::*;
use crate::{
    activity::{public_page, ActivityStore, Change, ConsumerState, ObservedActivity},
    delivery::{custom::CustomDelivery, engine::DeliveryEngine, relay_client::RelayClient},
    ingress::routing_store::test_server::Server,
    providers::ResponsesEvent,
    telegram::media_fixture::{Answer, Fixture},
    turns::{observer::NoopObserver, TurnTracker},
    DeliveryConfig,
};
use cmaeg_core::seal::ReplyAddress;
use reqwest::StatusCode;
use std::time::{Duration, Instant};

#[tokio::test]
async fn split_answer_replay_preserves_partial_and_confirmed_outcomes_without_more_sends() {
    for partial in [true, false] {
        let setup = topic_setup().await;
        let platform = Fixture::new().await;
        platform.answer(accepted_part(800));
        platform.answer(if partial {
            Answer::json(
                StatusCode::BAD_REQUEST,
                json!({"ok":false,"error_code":400,"description":"fixture refusal"}),
            )
        } else {
            accepted_part(801)
        });
        // The journal is clock-controlled. The Activity projection retains real timestamps.
        let activity_server = Server::start_realtime().await;
        let activity = Arc::new(ActivityStore::new(
            activity_server.records.clone(),
            Arc::new(fixture::keys()),
        ));
        let identity = &setup.plan.identity;
        let target = setup.target();
        let record = identity.record_id.as_str();
        let event = identity.event_id.as_str();
        activity
            .received_at(
                &identity.owner,
                record,
                &setup.plan.activity,
                time::OffsetDateTime::now_utc(),
            )
            .await
            .unwrap();
        const RESPONSE: &str = "resp_1";
        const ITEM: &str = "primary-split-answer";
        activity
            .change(
                record,
                event,
                &Change::state(
                    target.launch.consumer.as_str(),
                    ConsumerState::Accepted,
                    Some(RESPONSE.into()),
                ),
            )
            .await
            .unwrap();
        let text = format!("{}{}", "x".repeat(4096), "end of split answer");
        let mut frame = recovery_frame(RESPONSE, 0, false);
        frame.event = ResponsesEvent::OutputTextDone {
            item_id: ITEM.into(),
            text: text.clone(),
        };
        frame.raw = json!({
            "type":"response.output_text.done","response_id":RESPONSE,
            "sequence_number":0,"item_id":ITEM,"text":text,
        });
        let effect = ReplayEffect::Reply(Box::new(
            serde_json::from_value(json!({"text":text})).unwrap(),
        ));

        let mut lease = accepted(&setup).await;
        let (delivery, engine) = answer_lane(&setup, &platform, &activity);
        let initial = delivery
            .deliver_frame(identity, &target.id, &mut lease, &frame, effect.clone())
            .await
            .unwrap();
        assert!(engine.drain(Instant::now() + Duration::from_secs(5)).await);
        drop(delivery);

        let calls = platform.calls();
        assert_eq!(calls.len(), 2);
        let sent = calls
            .iter()
            .map(|call| {
                assert!(call.path.ends_with("/sendMessage"));
                let body: serde_json::Value = serde_json::from_slice(&call.body).unwrap();
                assert_eq!(body["chat_id"], "-42");
                assert_eq!(body["message_thread_id"], "7");
                body["text"].as_str().unwrap().to_owned()
            })
            .collect::<String>();
        assert_eq!(sent, text);

        let before = stored(&setup).await;
        assert_eq!(cursor(&before), (!partial).then_some(0));
        let checkpoint = before.delivery.as_ref().unwrap();
        assert_eq!(checkpoint.response_id, RESPONSE);
        assert_eq!(checkpoint.cursor, frame.sequence_number);
        assert_eq!(
            checkpoint.frame_hash,
            storage::hash(&storage::bytes(&frame.raw).unwrap())
        );
        assert_eq!(
            checkpoint.effect_hash,
            storage::hash(&storage::bytes(&effect).unwrap())
        );
        let retained = match (&checkpoint.status, partial) {
            (DeliveryStatus::Partial { receipt }, true)
            | (
                DeliveryStatus::Settled {
                    receipt: Some(receipt),
                },
                false,
            ) => receipt,
            _ => panic!("unexpected delivery checkpoint"),
        };
        let operation = storage::hash(
            &storage::bytes(&(
                &target.id,
                RESPONSE,
                checkpoint.cursor,
                &checkpoint.frame_hash,
                &checkpoint.effect_hash,
            ))
            .unwrap(),
        );
        let receipt = setup
            .journal()
            .receipts(&operation, &before.owner, before.retained_until)
            .load(retained)
            .await
            .unwrap();
        assert_answer_result(initial, partial, &receipt);
        assert_eq!(
            receipt.delivery,
            if partial {
                DeliveryOutcome::Unknown {
                    reason: "partial_delivery".into(),
                }
            } else {
                DeliveryOutcome::Confirmed {
                    platform_message_id: Some("800".into()),
                }
            }
        );
        let expected_ids = if partial {
            &["800"][..]
        } else {
            &["800", "801"][..]
        };
        assert_eq!(receipt.message_parts.len(), expected_ids.len());
        for (index, (part, id)) in receipt.message_parts.iter().zip(expected_ids).enumerate() {
            assert_eq!(usize::from(part.index), index);
            let reference = part.message_ref.open(&fixture::keys()).unwrap().payload;
            assert_eq!(reference.platform_message_id, *id);
            assert_eq!(reference.event_id, identity.event_id);
            assert_eq!(reference.channel_id, identity.channel_id);
            // The default consumer keeps the legacy omission in sealed references.
            assert_eq!(
                reference.consumer.as_ref(),
                Some(&target.launch.consumer).filter(|consumer| !consumer.is_default())
            );
            assert_eq!(reference.route_id, "telegram:77:-42");
        }
        let snapshot = public_page(&activity.latest(&identity.owner, record).await.unwrap());
        let observation = &snapshot.items[0].consumers[0];
        assert_eq!(observation.response_id.as_deref(), Some(RESPONSE));
        assert_eq!(observation.deliveries.len(), 1);
        let observed = &observation.deliveries[0];
        assert_eq!(observed.step, "answer");
        assert_eq!(
            observed.outcome,
            if partial { "unknown" } else { "confirmed" }
        );
        assert_eq!(
            observed.answer_response_id.as_deref(),
            (!partial).then_some(RESPONSE)
        );
        assert_eq!(observed.confirmed_at.is_some(), !partial);
        // The committed frame supplies exact production time even when only
        // part of the answer reached the platform.
        assert!(observation.produced_at.is_some());
        assert!(observation.produced_by.is_none());

        setup.journal().release(&mut lease).await.unwrap();
        let mut resumed = accepted(&setup).await;
        let (restarted, engine) = answer_lane(&setup, &platform, &activity);
        let mut changed = frame.clone();
        changed.raw["item_id"] = json!("different-item");
        changed.event = ResponsesEvent::OutputTextDone {
            item_id: "different-item".into(),
            text,
        };
        assert!(matches!(
            restarted
                .deliver_frame(identity, &target.id, &mut resumed, &changed, effect.clone())
                .await,
            Err(ExecutionError::Invalid)
        ));
        let replay = restarted
            .deliver_frame(identity, &target.id, &mut resumed, &frame, effect)
            .await
            .unwrap();
        assert_answer_result(replay, partial, &receipt);
        assert!(engine.drain(Instant::now() + Duration::from_secs(5)).await);
        let after = stored(&setup).await;
        assert_eq!(cursor(&after), cursor(&before));
        assert_eq!(
            storage::bytes(&after.delivery).unwrap(),
            storage::bytes(&before.delivery).unwrap()
        );
        let after_activity = public_page(&activity.latest(&identity.owner, record).await.unwrap());
        assert_eq!(json!(after_activity), json!(snapshot));
        assert_eq!(platform.calls().len(), 2);
    }
}

async fn topic_setup() -> Setup {
    let mut activity = fixture::message("Deliver the whole answer");
    activity.conversation.id = "-42".into();
    activity.conversation.thread_id = Some("7".into());
    activity.conversation.kind = cmaeg_core::activity::ConversationKind::Group;
    let mut request = fixture::request(
        |channel| {
            channel["sources"] = json!([
                {"type":"telegram","bot_id":"77","admission":{"type":"all"}}
            ]);
            channel["secrets"]["telegram_bot_token"] = "77:fixture".into();
            channel["routing"]["default_targets"] = json!(["default"]);
            channel["reply"]["sinks"] = json!([{"type":"source"}]);
        },
        activity,
    );
    fixture::rewrite_ref(&mut request, |event| {
        let ReplyAddress::Telegram { thread_id, .. } = &mut event.reply else {
            panic!("fixture must target the original Telegram topic");
        };
        *thread_id = Some("7".into());
    });
    Setup::with_request(supported(), request).await
}

fn accepted_part(id: u16) -> Answer {
    Answer::json(
        StatusCode::OK,
        json!({"ok":true,"result":{"message_id":id,"chat":{"id":-42},"message_thread_id":7}}),
    )
}

fn answer_lane(
    setup: &Setup,
    platform: &Fixture,
    activity: &Arc<ActivityStore>,
) -> (RoutedDelivery, DeliveryEngine) {
    let (mut delivery, receiver) = lane(setup);
    delivery.journal = delivery.journal.with_activity(activity.clone());
    let relay = RelayClient::new("https://nyx.example", Duration::from_secs(5))
        .unwrap()
        .with_telegram(platform.api.clone())
        .with_native_split_text_refs(true);
    let engine = DeliveryEngine::start_observed(
        receiver,
        Arc::new(fixture::keys()),
        relay,
        DeliveryConfig::default(),
        TurnTracker::new(Default::default()),
        CustomDelivery::new("https://gateway.example".into(), Arc::default()),
        Arc::new(ObservedActivity::new(
            activity.clone(),
            Arc::new(NoopObserver),
        )),
    );
    (delivery, engine)
}

fn assert_answer_result(result: FrameDelivery, partial: bool, retained: &IntentReceipt) {
    match (result, partial) {
        (FrameDelivery::Unknown, true) => {}
        (
            FrameDelivery::Settled {
                cursor: 0,
                receipt: Some(receipt),
            },
            false,
        ) => {
            assert_eq!(
                storage::bytes(&receipt).unwrap(),
                storage::bytes(retained).unwrap()
            );
        }
        _ => panic!("unexpected answer replay result"),
    }
}
