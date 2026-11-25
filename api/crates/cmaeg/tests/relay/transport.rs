use super::support::*;
use cmaeg::IngressConfig;
use serde_json::Value;
use std::{
    sync::atomic::Ordering,
    time::{Duration, Instant},
};

#[tokio::test]
async fn relay_command_addressing_rejects_unknown_and_preserves_default_controls() {
    for command in [true, false] {
        for (text, refused) in [
            ("/to absent-fixture hello", command),
            ("/to default hello", false),
            ("ordinary message", false),
        ] {
            let fixture = Fixture::new(
                |value| {
                    value["routing"] = serde_json::json!({
                        "version": 1,
                        "default_consumer": {"name": "default"},
                        "default_targets": ["default"],
                        "addressing": {"command": command}
                    });
                },
                Default::default(),
            )
            .await;
            fixture.recorder.outcome.store(1, Ordering::SeqCst);
            let mut envelope = Envelope::new();
            envelope.payload["content"]["text"] = text.into();
            envelope.payload["raw_platform_data"]["message"]["text"] = text.into();
            let response = fixture.send(&envelope).await;
            if refused {
                problem(response, 403, "target_unknown").await;
                assert!(fixture.recorder.requests.lock().unwrap().is_empty());
            } else {
                assert_eq!(response.status().as_u16(), 202);
                let requests = fixture.recorder.requests.lock().unwrap();
                assert_eq!(requests.len(), 1);
                assert!(requests[0].consumer.is_none());
            }
        }
    }
}

#[tokio::test]
async fn rejects_missing_duplicate_invalid_and_mismatched_headers() {
    let fixture = Fixture::new(|_| {}, Default::default()).await;
    for header in [
        "x-nyxid-callback-token",
        "x-nyxid-message-id",
        "x-nyxid-platform",
        "x-nyxid-timestamp",
    ] {
        let (body, token) = Envelope::new().signed();
        let mut request = fixture.request(body.clone(), &token).build().unwrap();
        request.headers_mut().remove(header);
        problem(
            fixture.client.execute(request).await.unwrap(),
            401,
            "header_missing",
        )
        .await;
        problem(
            fixture
                .request(body, &token)
                .header(header, "extra")
                .send()
                .await
                .unwrap(),
            401,
            "header_duplicate",
        )
        .await;
    }
    for (header, value, status, reason) in [
        ("x-nyxid-platform", "other", 400, "no matching relay source"),
        ("x-nyxid-message-id", "other", 401, "message_mismatch"),
        (
            "x-nyxid-timestamp",
            "2020-01-01T00:00:00Z",
            401,
            "timestamp_mismatch",
        ),
        ("x-nyxid-timestamp", "12345", 401, "timestamp_mismatch"),
        ("x-nyxid-platform", "telegram,other", 401, "header_invalid"),
        ("x-nyxid-message-id", "", 401, "header_invalid"),
    ] {
        let (body, token) = Envelope::new().signed();
        let mut request = fixture.request(body, &token).build().unwrap();
        request.headers_mut().insert(header, value.parse().unwrap());
        problem(
            fixture.client.execute(request).await.unwrap(),
            status,
            reason,
        )
        .await;
    }
}
#[tokio::test]
async fn enforces_media_and_streaming_body_limit_before_restoring() {
    let fixture = Fixture::new(
        |_| {},
        IngressConfig {
            max_body: 64,
            ..Default::default()
        },
    )
    .await;
    let response = fixture
        .client
        .post(&fixture.url)
        .body("{}")
        .send()
        .await
        .unwrap();
    problem(response, 415, "application/json is required").await;
    let response = fixture
        .client
        .post(&fixture.url)
        .header("content-type", "application/json")
        .body(vec![b' '; 65])
        .send()
        .await
        .unwrap();
    problem(response, 413, "callback body exceeds limit").await;
    // Unknown size forces the server's streaming limit instead of Content-Length precheck.
    let chunks =
        futures_util::stream::iter([Ok::<_, std::io::Error>(vec![b' '; 32]), Ok(vec![b' '; 33])]);
    let response = fixture
        .client
        .post(&fixture.url)
        .header("content-type", "application/json")
        .body(reqwest::Body::wrap_stream(chunks))
        .send()
        .await
        .unwrap();
    problem(response, 413, "callback body exceeds limit").await;
    assert_eq!(fixture.backend.records.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn exhausted_capacity_refuses_immediately_and_recovers() {
    let fixture = Fixture::new(
        |_| {},
        IngressConfig {
            max_inflight: 1,
            ..Default::default()
        },
    )
    .await;
    fixture.recorder.outcome.store(2, Ordering::SeqCst);
    let (body, token) = Envelope::new().signed();
    let pending = tokio::spawn(fixture.request(body, &token).send());
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if !fixture.recorder.requests.lock().unwrap().is_empty() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let started = Instant::now();
    let response = fixture.send(&Envelope::new()).await;
    assert!(started.elapsed() < Duration::from_millis(100));
    assert_eq!(response.headers()["retry-after"], "1");
    problem(response, 429, "ingress capacity exhausted").await;
    fixture.recorder.wait.notify_one();
    assert_eq!(pending.await.unwrap().unwrap().status(), 202);
    fixture.recorder.outcome.store(0, Ordering::SeqCst);
    assert_eq!(fixture.send(&Envelope::new()).await.status(), 202);
}
#[tokio::test]
async fn warm_verification_is_local_and_total_includes_launcher_wait() {
    let fixture = Fixture::new(|_| {}, Default::default()).await;
    assert_eq!(fixture.send(&Envelope::new()).await.status(), 202);
    for _ in 0..10 {
        let response = fixture.send(&Envelope::new()).await;
        assert_eq!(response.status(), 202);
        let timing = timing(&response);
        let duration = timing["restore"] + timing["verify"];
        assert_eq!(timing["launch"], 0.0); // This fixture uses a recording launcher, with no socket.
        assert!(duration < 50.0, "warm ingress took {duration} ms");
    }
    assert_eq!(fixture.backend.gets.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.backend.records.load(Ordering::SeqCst), 1);
    fixture.recorder.outcome.store(3, Ordering::SeqCst);
    problem(
        fixture.send(&Envelope::new()).await,
        503,
        "provider offline",
    )
    .await;
    fixture.recorder.outcome.store(2, Ordering::SeqCst);
    let (body, token) = Envelope::new().signed();
    let before = fixture.recorder.requests.lock().unwrap().len();
    let pending = tokio::spawn(fixture.request(body, &token).send());
    tokio::time::timeout(Duration::from_secs(2), async {
        while fixture.recorder.requests.lock().unwrap().len() == before {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    // Measure the complete wait after launcher entry, independently of client scheduling.
    let waiting = std::time::Instant::now();
    while let Some(remaining) = Duration::from_millis(100).checked_sub(waiting.elapsed()) {
        tokio::time::sleep(remaining).await;
    }
    fixture.recorder.wait.notify_one();
    let response = pending.await.unwrap().unwrap();
    let timing = timing(&response);
    let duration = timing["restore"] + timing["verify"];
    assert!(timing["total"] >= 100.0);
    assert!(duration < 50.0);
    assert_eq!(
        response.json::<Value>().await.unwrap()["status"],
        "accepted"
    );
}

fn timing(response: &reqwest::Response) -> std::collections::BTreeMap<String, f64> {
    let values: std::collections::BTreeMap<String, f64> = response.headers()["server-timing"]
        .to_str()
        .unwrap()
        .split(',')
        .map(|part| {
            let (name, value) = part.trim().split_once(";dur=").unwrap();
            (
                name.to_owned(),
                value.split(';').next().unwrap().parse().unwrap(),
            )
        })
        .collect();
    for name in ["restore", "verify", "launch"] {
        assert!(values["total"] >= values[name]);
    }
    values
}

#[test]
fn telegram_new_platform_alias_maps_to_telegram_protocol() {
    use std::str::FromStr;
    assert_eq!(
        cmaeg_core::channel::Platform::from_str("telegram-new").unwrap(),
        cmaeg_core::channel::Platform::Telegram
    );
}

#[tokio::test]
async fn accepts_managed_telegram_new_platform_alias() {
    let fixture = Fixture::new(|_| {}, Default::default()).await;
    // NyxID stamps the managed bot's own platform on every relay artifact:
    // callback claims, header, payload and the reply token.
    let mut envelope = Envelope::new();
    envelope.payload["platform"] = "telegram-new".into();
    envelope.callback["platform"] = "telegram-new".into();
    envelope.reply["platform"] = "telegram-new".into();
    let (body, token) = envelope.signed();
    let mut request = fixture.request(body, &token).build().unwrap();
    request
        .headers_mut()
        .insert("x-nyxid-platform", "telegram-new".parse().unwrap());
    let response = fixture.client.execute(request).await.unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::ACCEPTED);
}

#[tokio::test]
async fn relay_callback_releases_request_body_before_launch() {
    use axum::extract::{Path, State};
    let fixture = Fixture::new(|_| {}, Default::default()).await;
    let (bytes, token) = Envelope::new().signed();
    let (body, dropped) = probed_body(bytes);
    fixture.recorder.expect_released(dropped.clone());
    let response = cmaeg::http::callbacks::callback(
        State(fixture.state.clone()),
        Path(fixture.id.to_string()),
        fixture.local_request(body, &token),
    )
    .await;
    assert_eq!(response.status(), 202);
    assert_eq!(fixture.recorder.probed.load(Ordering::SeqCst), 1);
    assert!(dropped.load(Ordering::SeqCst));
}
