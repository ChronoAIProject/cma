//! Real executable, signed HTTP callback and process-signal evidence for runner composition.
use super::{
    fake_provider::{FakeProvider, FakeProviderConfig},
    runner_process::Process,
    support::{Envelope, Fixture},
};
use serde_json::{json, Value};
use std::{
    fs,
    time::{Duration, Instant},
};
use tokio::process::Command;
#[tokio::test]
async fn executable_delivers_ack_final_and_tool_reply_and_drains_the_relay_without_secret_logs() {
    use sha2::{Digest, Sha256};
    for use_tool in [false, true] {
        let provider = FakeProvider::start(FakeProviderConfig {
            terminal_delay: Duration::from_secs(5),
            text: "final answer".into(),
            ..Default::default()
        })
        .await;
        let relay = super::fake_nyxid_reply::FakeReply::start(vec![]).await;
        let agent_key = format!("nyx_{}", uuid::Uuid::new_v4().simple());
        let mut fixture = Fixture::new(
            |value| {
                value["reply"]["ack_after_ms"] = 1000.into();
                value["secrets"]["provider_credential"] = agent_key.clone().into();
                value["secrets"]["agent_key_hash"] =
                    format!("sha256:{:x}", Sha256::digest(agent_key.as_bytes())).into();
                value["secrets"]["channel_key"] = "test-route-credential".into();
            },
            Default::default(),
        )
        .await;
        let mut process = Process::with_reply(&mut fixture, &provider, 20000, &relay.base).await;
        assert_eq!(fixture.send(&Envelope::new()).await.status(), 202);
        if use_tool {
            let request: Value = serde_json::from_str(
                &provider
                    .requests()
                    .into_iter()
                    .find(|request| request.method == "POST")
                    .unwrap()
                    .body,
            )
            .unwrap();
            let reference = request["event_context"]["event_ref"].as_str().unwrap();
            let response = fixture
                .client
                .post(format!("{}/v1/events/{reference}/replies", process.base))
                .bearer_auth(&agent_key)
                .json(&json!({"text":"tool answer"}))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 200);
            assert_eq!(
                response.json::<Value>().await.unwrap()["delivery"]["outcome"],
                "confirmed"
            );
        } else {
            relay.wait(1).await;
            assert_eq!(relay.texts(), ["Working on it…"]);
        }
        process.terminate().await;
        assert!(
            tokio::time::timeout(Duration::from_secs(9), process.child.wait())
                .await
                .unwrap()
                .unwrap()
                .success()
        );
        if use_tool {
            assert_eq!(relay.texts(), ["tool answer"]);
        } else {
            assert_eq!(relay.texts(), ["Working on it…", "final answer"]);
        }
        process.assert_safe_logs();
        assert!(!process.logs().contains(&agent_key));
        assert!(!process.logs().contains("final answer"));
        assert!(!process.logs().contains("tool answer"));
        provider.close().await;
    }
}
fn credential(value: &mut Value) {
    value["secrets"]["provider_credential"] = "runtime-test-provider-bearer".into();
    value["reply"]["ack_after_ms"] = 0.into();
    value["reply"]["auto_final"] = false.into();
}

#[tokio::test]
async fn executable_accepts_signed_custom_events_and_curl_streams_through_completion() {
    let provider = FakeProvider::start(FakeProviderConfig {
        terminal_delay: Duration::from_millis(200),
        ..Default::default()
    })
    .await;
    let secret = cmaeg_core::channel::Secret::new(uuid::Uuid::new_v4().to_string());
    let mut fixture = Fixture::new(
        |value| {
            credential(value);
            value["sources"] = json!([{"type":"custom","auth":{"type":"hmac","key_id":"ingress"}}]);
            value["partition"]["key"] = "per_event".into();
            value["reply"]["sinks"] = json!([{"type":"sync"}]);
            value["secrets"]["hmac_secrets"] = json!([["ingress", secret.as_str()]]);
        },
        Default::default(),
    )
    .await;
    let mut process = Process::start(&mut fixture, &provider, 20000).await;
    let body = json!({"specversion":"1.0","id":"event-1","type":"test.event","source":"https://events.example","data":{"message":"hello"}}).to_string();
    let body_path = process.dir.join("event.json");
    fs::write(&body_path, &body).unwrap();
    for stream in [true, false] {
        let timestamp = time::OffsetDateTime::now_utc().unix_timestamp().to_string();
        let signature =
            cmaeg::auth::webhook::signature(&secret, "delivery-1", &timestamp, body.as_bytes());
        let url = format!(
            "{}/v1/channels/{}/events{}",
            process.base,
            fixture.id,
            if stream { "?mode=stream" } else { "" }
        );
        let output = Command::new("curl")
            .args([
                "--silent",
                "--show-error",
                "--fail",
                "--max-time",
                "5",
                "--request",
                "POST",
                "--header",
                "Content-Type: application/cloudevents+json",
                "--header",
                "webhook-id: delivery-1",
                "--header",
                &format!("webhook-timestamp: {timestamp}"),
                "--header",
                &format!("webhook-signature: {signature}"),
                "--data-binary",
                &format!("@{}", body_path.display()),
                &url,
            ])
            .output()
            .await
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let text = String::from_utf8(output.stdout).unwrap();
        if stream {
            assert!(text.contains("event: cmaeg.text.delta"));
            assert!(text.contains("event: cmaeg.completed"));
        } else {
            assert_eq!(
                serde_json::from_str::<Value>(&text).unwrap()["status"],
                "accepted"
            );
        }
    }
    process.terminate().await;
    assert!(
        tokio::time::timeout(Duration::from_secs(3), process.child.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
    process.assert_safe_logs();
    assert!(!process.logs().contains(secret.as_str()));
    provider.close().await;
}

#[tokio::test]
async fn real_signed_callback_accepts_at_two_seconds_and_defers_at_twenty() {
    for (delay, budget, expected) in [(2, 20000, 2), (30, 20000, 20)] {
        let fake = FakeProvider::start(FakeProviderConfig {
            response_delay: Duration::from_secs(delay),
            ..Default::default()
        })
        .await;
        let mut fixture = Fixture::new(credential, Default::default()).await;
        let mut process = Process::start(&mut fixture, &fake, budget).await;
        let start = Instant::now();
        let response = fixture.send(&Envelope::new()).await;
        assert_eq!(response.status(), 202);
        assert!(start.elapsed() >= Duration::from_secs(expected));
        assert!(start.elapsed() < Duration::from_secs(expected + 2));
        assert_eq!(
            response.json::<Value>().await.unwrap()["status"],
            "accepted"
        );
        tokio::time::timeout(Duration::from_secs(12), async {
            while !process.logs().contains("outcome=\"completed\"") {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            fake.requests()
                .iter()
                .filter(|r| r.method == "POST")
                .count(),
            1
        );
        process.assert_safe_logs();
        process.terminate().await;
        assert!(
            tokio::time::timeout(Duration::from_secs(2), process.child.wait())
                .await
                .unwrap()
                .unwrap()
                .success()
        );
        fake.close().await;
    }
}
#[cfg(unix)]
#[tokio::test]
async fn sigterm_keeps_process_alive_until_ten_second_turn_finishes() {
    let fake = FakeProvider::start(FakeProviderConfig {
        terminal_delay: Duration::from_secs(10),
        ..Default::default()
    })
    .await;
    let mut fixture = Fixture::new(credential, Default::default()).await;
    let mut process = Process::start(&mut fixture, &fake, 20000).await;
    assert_eq!(fixture.send(&Envelope::new()).await.status(), 202);
    let busy = fixture.send(&Envelope::new()).await;
    assert_eq!(busy.status(), 503);
    assert_eq!(busy.headers()["retry-after"], "2");
    assert_eq!(busy.json::<Value>().await.unwrap()["code"], "provider_busy");
    let start = Instant::now();
    process.terminate().await;
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let ready = reqwest::get(format!("{}/readyz", process.base))
                .await
                .unwrap();
            if ready.status() == 503 {
                assert!(ready.json::<Value>().await.unwrap()["reasons"]
                    .as_array()
                    .unwrap()
                    .contains(&json!("draining")));
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let refused = fixture.send(&Envelope::new()).await;
    assert_eq!(refused.status(), 503);
    assert_eq!(refused.json::<Value>().await.unwrap()["code"], "draining");
    assert!(process.child.try_wait().unwrap().is_none());
    assert!(
        tokio::time::timeout(Duration::from_secs(12), process.child.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
    assert!(start.elapsed() >= Duration::from_secs(9));
    assert!(process.logs().contains("outcome=\"completed\""));
    process.assert_safe_logs();
    fake.close().await;
}

#[tokio::test]
async fn nyxid_photo_callback_is_refused_not_rejected_by_the_real_binary() {
    use super::nyxid_shapes::{telegram_photo, TELEGRAM_FILE_ID};
    let provider = FakeProvider::start(FakeProviderConfig::default()).await;
    let mut fixture = Fixture::new(credential, Default::default()).await;
    let mut process = Process::start(&mut fixture, &provider, 20000).await;
    let response = fixture.send(&telegram_photo()).await;
    assert_eq!(response.status(), 202);
    assert_eq!(
        response.json::<Value>().await.unwrap(),
        json!({"status": "refused", "reason": "attachment_unsupported"})
    );
    tokio::time::timeout(Duration::from_secs(5), async {
        while !process.logs().contains("status=202") {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert!(provider.requests().iter().all(|r| r.method != "POST"));
    let logs = process.logs();
    for private in [
        TELEGRAM_FILE_ID,
        "/api/v1/channel-relay/messages/",
        "Mira Fixture",
        "look at this",
    ] {
        assert!(!logs.contains(private), "attachment fact appeared in trace");
    }
    process.assert_safe_logs();
    process.terminate().await;
    assert!(
        tokio::time::timeout(Duration::from_secs(3), process.child.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
    provider.close().await;
}
