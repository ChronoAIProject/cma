use super::*;
use std::time::{Duration, Instant};

async fn send_test(client: &Client, base: &str, channel: &serde_json::Value) -> reqwest::Response {
    client
        .post(format!(
            "{base}/v1/channels/{}/test",
            channel["channel_id"].as_str().unwrap()
        ))
        .bearer_auth("token-alice")
        .send()
        .await
        .unwrap()
}

#[tokio::test]
async fn busy_retries_preserve_exact_request_and_respect_minimum_delay() {
    for (retry_after, minimum) in [
        (Some(1), Duration::from_secs(1)),
        (None, Duration::from_millis(500)),
        (Some(0), Duration::from_millis(500)),
    ] {
        let provider = FakeProvider::start(FakeProviderConfig {
            busy_once: true,
            busy_retry_after: retry_after,
            ..Default::default()
        })
        .await;
        let cma = FakeCma::start(false).await;
        let nyxid = FakeNyxid::start().await;
        let (base, stop, task) = app(&provider, &cma, &nyxid).await;
        let client = Client::new();
        let channel = created(&client, &base).await;
        let response = send_test(&client, &base, &channel).await;
        assert_eq!(response.status(), 200);
        let body: serde_json::Value = response.json().await.unwrap();
        assert_eq!(body["first_text"], "hello");
        let requests = provider.requests();
        let turns: Vec<_> = requests.iter().filter(|r| r.method == "POST").collect();
        assert_eq!(turns.len(), 2);
        assert!(turns[0]
            .idempotency_key
            .as_ref()
            .unwrap()
            .starts_with("test_"));
        assert_eq!(turns[0].idempotency_key, turns[1].idempotency_key);
        assert_eq!(turns[0].body, turns[1].body);
        assert_eq!(turns[0].authorization, turns[1].authorization);
        assert_eq!(turns[0].path, turns[1].path);
        assert!(turns[1].received_at.duration_since(turns[0].received_at) >= minimum);
        assert_eq!(
            requests
                .iter()
                .filter(|r| r.path.contains("/conversations/"))
                .count(),
            1
        );
        stop.send(()).unwrap();
        task.await.unwrap();
        provider.close().await;
        cma.close().await;
        nyxid.close().await;
    }
}

#[tokio::test]
async fn busy_retry_after_cannot_extend_the_thirty_second_test_deadline() {
    let provider = FakeProvider::start(FakeProviderConfig {
        busy_always: true,
        busy_retry_after: Some(60),
        ..Default::default()
    })
    .await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    let (base, stop, task) = app(&provider, &cma, &nyxid).await;
    let client = Client::new();
    let channel = created(&client, &base).await;
    let started = Instant::now();
    let response =
        tokio::time::timeout(Duration::from_secs(35), send_test(&client, &base, &channel))
            .await
            .expect("management test exceeded its original deadline");
    assert_eq!(response.status(), 503);
    assert!(started.elapsed() >= Duration::from_secs(30));
    let problem: serde_json::Value = response.json().await.unwrap();
    assert_eq!(problem["code"], "provider_unavailable");
    assert_eq!(
        provider
            .requests()
            .iter()
            .filter(|r| r.method == "POST")
            .count(),
        1
    );
    stop.send(()).unwrap();
    task.await.unwrap();
    provider.close().await;
    cma.close().await;
    nyxid.close().await;
}

#[tokio::test]
async fn unavailable_and_stream_failures_never_retry_the_test_turn() {
    let accepted = json!({"type":"response.created","response":{"id":"resp_fake"}});
    let cases = [
        (
            "ambiguous header timeout",
            FakeProviderConfig {
                header_delay: Duration::from_secs(3),
                ..Default::default()
            },
            1,
        ),
        (
            "disconnect before acceptance",
            FakeProviderConfig {
                disconnect_after: Some(0),
                ..Default::default()
            },
            1,
        ),
        (
            "disconnect after busy then acceptance",
            FakeProviderConfig {
                busy_once: true,
                disconnect_after: Some(2),
                ..Default::default()
            },
            2,
        ),
        (
            "failed stream is not a busy refusal",
            FakeProviderConfig {
                frames: Some(vec![
                    accepted.clone(),
                    json!({"type":"response.failed","error":{"code":"session_busy"}}),
                ]),
                ..Default::default()
            },
            1,
        ),
        (
            "incomplete stream",
            FakeProviderConfig {
                frames: Some(vec![accepted, json!({"type":"response.incomplete"})]),
                ..Default::default()
            },
            1,
        ),
    ];
    for (label, config, expected_turns) in cases {
        let provider = FakeProvider::start(config).await;
        let cma = FakeCma::start(false).await;
        let nyxid = FakeNyxid::start().await;
        let (base, stop, task) = app(&provider, &cma, &nyxid).await;
        let client = Client::new();
        let channel = created(&client, &base).await;
        let response =
            tokio::time::timeout(Duration::from_secs(5), send_test(&client, &base, &channel))
                .await
                .expect(label);
        assert_eq!(response.status(), 503, "{label}");
        assert_eq!(
            provider
                .requests()
                .iter()
                .filter(|r| r.method == "POST")
                .count(),
            expected_turns,
            "{label}"
        );
        stop.send(()).unwrap();
        task.await.unwrap();
        provider.close().await;
        cma.close().await;
        nyxid.close().await;
    }
}
