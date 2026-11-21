use super::{fake_provider::FakeProviderConfig, receiver::Receiver, support::App};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use cmaeg::delivery::{
    intents::DeliveryOutcome,
    sync::SyncRegistry,
    webhook_client::{public_ip, WebhookClient},
};
use cmaeg_core::{
    channel::Secret,
    seal::{EventRef, ReplyAddress},
};
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::{
    sync::{atomic::Ordering, Arc},
    time::Duration,
};

#[tokio::test]
async fn streams_deltas_committed_messages_tool_messages_and_terminal_then_closes() {
    let frames = vec![
        json!({"type":"response.created","response":{"id":"response"}}),
        json!({"type":"response.output_text.delta","item_id":"m","delta":"hello"}),
        json!({"type":"response.output_text.done","item_id":"m","text":"hello"}),
        json!({"type":"response.completed","response":{"id":"response"}}),
    ];
    let mut app = App::new(
        |_| {},
        FakeProviderConfig {
            frames: Some(frames),
            terminal_delay: Duration::from_millis(500),
            ..Default::default()
        },
        Default::default(),
        0,
    )
    .await;
    let response = app
        .request(
            &format!("{}?mode=stream", app.url),
            app.event("event-1").to_string(),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert!(response.headers()["content-type"]
        .to_str()
        .unwrap()
        .starts_with("text/event-stream"));
    let body: Value = serde_json::from_str(
        &app.provider
            .requests()
            .into_iter()
            .find(|request| request.method == "POST")
            .unwrap()
            .body,
    )
    .unwrap();
    let reference = body["event_context"]["event_ref"].as_str().unwrap();
    let tool = app.fixture.client.post(format!("{}/v1/events/{reference}/replies", app.base)).bearer_auth(&app.key)
        .json(&json!({"blocks":[{"type":"buttons","id":"choice","options":[{"id":"one","label":"One"}]}]})).send().await.unwrap();
    assert_eq!(tool.status(), 200);
    assert_eq!(
        tool.json::<Value>().await.unwrap()["rendering"]["blocks"][0]["outcome"]["type"],
        "exact"
    );
    let text = tokio::time::timeout(Duration::from_secs(3), response.text())
        .await
        .unwrap()
        .unwrap();
    assert!(text.contains("event: cmaeg.text.delta"));
    assert_eq!(text.matches("event: cmaeg.message").count(), 2);
    assert!(text.contains("event: cmaeg.completed"));
    assert!(text.find("cmaeg.text.delta").unwrap() < text.find("cmaeg.completed").unwrap());
    assert!(!text.contains(app.secret.as_str()));
    assert!(!text.contains(&app.key));
    app.drain().await;
    assert!(app.waiters.is_empty());
    assert_eq!(
        app.receiver
            .events()
            .iter()
            .map(|event| event["type"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["cmaeg.response.message", "cmaeg.response.completed"]
    );
}
#[tokio::test]
async fn result_webhooks_sign_exact_bytes_retry_with_stable_identity_and_survive_missing_local_waiter(
) {
    let mut app = App::new(|_| {}, Default::default(), Default::default(), 1).await;
    assert_eq!(
        app.request(&app.url, app.event("event-1").to_string())
            .send()
            .await
            .unwrap()
            .status(),
        202
    );
    let body: Value = serde_json::from_str(
        &app.provider
            .requests()
            .into_iter()
            .find(|request| request.method == "POST")
            .unwrap()
            .body,
    )
    .unwrap();
    let reference: EventRef = body["event_context"]["event_ref"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let mut event = reference
        .open(&app.fixture.state.master_keys, super::relay_support::now())
        .unwrap()
        .payload;
    event.reply = ReplyAddress::Sync {
        waiter: uuid::Uuid::new_v4().to_string(),
    };
    let reference = EventRef::seal(&event, &app.fixture.state.master_keys).unwrap();
    let response = app
        .fixture
        .client
        .post(format!(
            "{}/v1/events/{}/replies",
            app.base,
            reference.as_str()
        ))
        .bearer_auth(&app.key)
        .json(&json!({"text":"late tool"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.json::<Value>().await.unwrap()["delivery"]["outcome"],
        "confirmed"
    );
    app.drain().await;
    let requests = app.receiver.received.lock().unwrap();
    assert_eq!(requests.len(), 3);
    assert_eq!(
        requests[0].headers["webhook-id"],
        requests[1].headers["webhook-id"]
    );
    assert_eq!(requests[0].body, requests[1].body);
    for request in requests.iter() {
        let mut signed = format!(
            "{}.{}.",
            request.headers["webhook-id"].to_str().unwrap(),
            request.headers["webhook-timestamp"].to_str().unwrap()
        )
        .into_bytes();
        signed.extend_from_slice(&request.body);
        let signature = STANDARD
            .decode(
                request.headers["webhook-signature"]
                    .to_str()
                    .unwrap()
                    .strip_prefix("v1,")
                    .unwrap(),
            )
            .unwrap();
        ring::hmac::verify(
            &ring::hmac::Key::new(ring::hmac::HMAC_SHA256, app.secret.as_str().as_bytes()),
            &signed,
            &signature,
        )
        .unwrap();
        let event: Value = serde_json::from_slice(&request.body).unwrap();
        assert!(event["source"]
            .as_str()
            .unwrap()
            .starts_with("https://gateway.example/channels/"));
        assert_eq!(event["subject"], "event-1");
        assert!(!String::from_utf8_lossy(&request.body).contains("cmaeg1."));
    }
    assert_eq!(app.receiver.transport.lookups.load(Ordering::SeqCst), 3);
}
#[tokio::test]
async fn webhook_policy_rejects_private_addresses_and_rechecks_dns_after_retry() {
    let receiver = Receiver::new(1).await;
    let client = WebhookClient::with_transport(receiver.transport.clone());
    let secret = Secret::new(uuid::Uuid::new_v4().to_string());
    for target in [
        "https://127.0.0.1/events",
        "https://10.0.0.1",
        "https://169.254.169.254",
        "https://[::1]",
        "https://[::ffff:127.0.0.1]",
        "http://receiver.example/events",
        "https://user:pass@receiver.example/events",
    ] {
        assert!(matches!(
            client.send(target, &secret, &json!({"id":"result"})).await,
            DeliveryOutcome::Rejected { .. }
        ));
    }
    for address in [
        "172.16.0.1",
        "192.168.0.1",
        "fc00::1",
        "fe80::1",
        "0.0.0.0",
        "100.64.0.1",
        "2002:7f00:1::",
    ] {
        assert!(!public_ip(address.parse().unwrap()));
    }
    let event = json!({"id":"result"});
    let future = client.send("https://receiver.example/events", &secret, &event);
    let result = tokio::join!(future, async {
        while receiver.received.lock().unwrap().is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        *receiver.transport.resolved.lock().unwrap() = vec!["127.0.0.1:443".parse().unwrap()];
    })
    .0;
    assert_eq!(
        result,
        DeliveryOutcome::Rejected {
            reason: "webhook_target_invalid".into()
        }
    );
    assert_eq!(receiver.received.lock().unwrap().len(), 1);
    assert_eq!(receiver.transport.lookups.load(Ordering::SeqCst), 2);
}
#[tokio::test]
async fn slow_or_disconnected_sync_clients_are_bounded_and_unregister() {
    let registry = Arc::new(SyncRegistry::default());
    let (id, waiter) = registry.register().unwrap();
    for _ in 0..8 {
        assert!(registry.emit(&id, "cmaeg.text.delta", &json!({"text":"hello"}), false));
    }
    assert!(!registry.emit(&id, "cmaeg.text.delta", &json!({"text":"overflow"}), false));
    assert!(registry.is_empty());
    let stream = waiter.stream();
    futures_util::pin_mut!(stream);
    assert!(stream.next().await.unwrap().is_ok());
    assert!(stream.next().await.is_none());
    let (_, waiter) = registry.register().unwrap();
    drop(waiter);
    assert!(registry.is_empty());
}

#[tokio::test]
async fn failed_and_incomplete_provider_turns_emit_terminal_stream_and_webhook_failures() {
    for terminal in [
        json!({"type":"response.failed","error":{"code":"test_failure","message":"private provider diagnostic"}}),
        json!({"type":"response.incomplete","response":{"incomplete_details":{"reason":"test_reason"}}}),
    ] {
        let mut app = App::new(
            |_| {},
            FakeProviderConfig {
                frames: Some(vec![
                    json!({"type":"response.created","response":{"id":"response"}}),
                    terminal,
                ]),
                ..Default::default()
            },
            Default::default(),
            0,
        )
        .await;
        let response = app
            .request(
                &format!("{}?mode=stream", app.url),
                app.event("event").to_string(),
            )
            .send()
            .await
            .unwrap();
        let text = response.text().await.unwrap();
        assert!(text.contains("event: cmaeg.failed"));
        assert!(!text.contains("private provider diagnostic"));
        app.drain().await;
        let events = app.receiver.events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["type"], "cmaeg.response.failed");
        assert_eq!(events[0]["data"]["error"]["code"], "provider_unavailable");
    }
}
