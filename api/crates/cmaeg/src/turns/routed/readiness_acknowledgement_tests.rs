use super::*;
use crate::delivery::intents::ReplySource;

fn cold_provider(http: &HttpServer) {
    let mut memory = http.memory.lock().unwrap();
    memory.config.post_status = 429;
    memory.config.post_error_body = Some(
        json!({"error":{"type":"invalid_request_error","code":"conversation_not_ready"}})
            .to_string(),
    );
}

fn request(ack_after_ms: u32, relay: bool) -> crate::ingress::launch::LaunchRequest {
    let mut activity = fixture::message("first message");
    if relay {
        activity.source = serde_json::from_value(json!({
            "type":"nyxid_relay","platform":"telegram","route_id":fixture::ROUTE
        }))
        .unwrap();
    }
    fixture::request(
        |channel| {
            if relay {
                channel["sources"][1] =
                    crate::channels::routing_fixture::relay("telegram", fixture::ROUTE);
            }
            channel["routing"]["default_targets"] = json!(["default"]);
            channel["reply"]["ack_after_ms"] = ack_after_ms.into();
            channel["reply"]["sinks"] = json!([{"type":"source"}]);
        },
        activity,
    )
}

#[tokio::test]
async fn cold_channel_acknowledgement_survives_restart_and_preserves_final_answer() {
    for relay in [false, true] {
        let setup = Setup::with_request(supported(), request(1500, relay)).await;
        let http = HttpServer::start().await;
        cold_provider(&http);
        let (first, mut sends, work) = executor(&setup, &http);
        let (result, ()) = tokio::join!(first.execute(&work), async {
            let reply = tokio::time::timeout(Duration::from_secs(2), command(&mut sends))
                .await
                .expect("progress acknowledgement while the conversation waits");
            assert_eq!(reply.source, ReplySource::Acknowledgement);
            assert_eq!(reply.event.reply, setup.target().launch.event.reply);
            assert!(!reply.message.thread_reply);
            assert_eq!(
                reply.message.text.as_deref(),
                Some(DeliveryConfig::default().ack_text.as_str())
            );
            let _permit = reply.gate.attempt().await.expect("live send authority");
            assert!(reply.gate.commit());
            reply
                .receipt
                .send(receipt_for(if relay {
                    DeliveryOutcome::Unknown {
                        reason: "transport_interrupted".into(),
                    }
                } else {
                    DeliveryOutcome::Confirmed {
                        platform_message_id: Some("progress".into()),
                    }
                }))
                .unwrap();
        });
        assert!(matches!(result, Disposition::Retry(_)));
        assert!(matches!(stored(&setup).await.phase, Phase::Prepared));
        drop(first);

        let (restarted, mut sends, work) = executor(&setup, &http);
        assert!(matches!(
            restarted.execute(&work).await,
            Disposition::Retry(_)
        ));
        assert!(
            sends.try_recv().is_err(),
            "restart must not resend progress"
        );

        {
            let mut memory = http.memory.lock().unwrap();
            memory.config.post_status = 200;
            memory.config.replay_frames = Some(frames());
        }
        let (result, ()) = tokio::join!(restarted.execute(&work), async {
            let reply = tokio::time::timeout(Duration::from_secs(2), command(&mut sends))
                .await
                .expect("automatic final answer");
            assert_eq!(reply.source, ReplySource::Stream);
            assert_eq!(reply.message.text.as_deref(), Some("hello world"));
            assert_eq!(reply.event.reply, setup.target().launch.event.reply);
            reply
                .receipt
                .send(receipt_for(DeliveryOutcome::Confirmed {
                    platform_message_id: Some("answer".into()),
                }))
                .unwrap();
        });
        assert!(matches!(result, Disposition::Settled));
        assert!(matches!(
            restarted.execute(&work).await,
            Disposition::Settled
        ));
        assert!(sends.try_recv().is_err());
        let posts = http
            .seen()
            .into_iter()
            .filter(|entry| entry.method == "POST")
            .collect::<Vec<_>>();
        assert_eq!(posts.len(), 3);
        assert!(posts.iter().all(|entry| entry.body == posts[0].body
            && entry.idempotency.is_some()
            && entry.idempotency == posts[0].idempotency));
    }
}

#[tokio::test]
async fn cold_channel_without_acknowledgement_opt_in_keeps_retrying() {
    let setup = Setup::with_request(supported(), request(0, true)).await;
    let http = HttpServer::start().await;
    cold_provider(&http);
    let (runner, mut sends, work) = executor(&setup, &http);
    assert!(matches!(runner.execute(&work).await, Disposition::Retry(_)));
    assert!(sends.try_recv().is_err());
}

#[tokio::test]
async fn ready_channel_sends_only_its_answer() {
    let setup = Setup::with_request(supported(), request(1500, true)).await;
    let http = HttpServer::start().await;
    http.memory.lock().unwrap().config.replay_frames = Some(frames());
    let (runner, mut sends, work) = executor(&setup, &http);
    let (result, ()) = tokio::join!(runner.execute(&work), async {
        let reply = tokio::time::timeout(Duration::from_secs(2), command(&mut sends))
            .await
            .expect("automatic final answer");
        assert_eq!(reply.source, ReplySource::Stream);
        reply
            .receipt
            .send(receipt_for(DeliveryOutcome::Confirmed {
                platform_message_id: Some("answer".into()),
            }))
            .unwrap();
    });
    assert!(matches!(result, Disposition::Settled));
    assert!(sends.try_recv().is_err());
}
