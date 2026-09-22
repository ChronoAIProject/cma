use super::{fake_provider::FakeProviderConfig, relay_support::Envelope, support::App};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

#[tokio::test]
async fn signed_callback_with_five_second_provider_sends_ack_then_final_and_drains() {
    let mut app = App::new(
        vec![],
        1000,
        Default::default(),
        FakeProviderConfig {
            terminal_delay: Duration::from_secs(5),
            text: "final answer".into(),
            ..Default::default()
        },
    )
    .await;
    let started = Instant::now();
    assert_eq!(app.fixture.send(&Envelope::new()).await.status(), 202);
    app.fake.wait(1).await;
    assert_eq!(app.fake.texts(), ["Working on it…"]);
    assert!(started.elapsed() >= Duration::from_secs(1));
    assert!(
        app.tracker
            .drain(Instant::now() + Duration::from_secs(8))
            .await
    );
    app.drain().await;
    assert_eq!(app.fake.texts(), ["Working on it…", "final answer"]);
    assert!(started.elapsed() >= Duration::from_secs(5));
}
#[tokio::test]
async fn agent_tool_reply_during_signed_callback_turn_produces_exactly_one_platform_send() {
    let mut app = App::new(
        vec![],
        3000,
        Default::default(),
        FakeProviderConfig {
            terminal_delay: Duration::from_secs(5),
            ..Default::default()
        },
    )
    .await;
    assert_eq!(app.fixture.send(&Envelope::new()).await.status(), 202);
    let request: Value = serde_json::from_str(
        &app.provider
            .requests()
            .into_iter()
            .find(|request| request.method == "POST")
            .unwrap()
            .body,
    )
    .unwrap();
    let reference = request["event_context"]["event_ref"].as_str().unwrap();
    let response = app
        .fixture
        .client
        .post(format!("{}/v1/events/{reference}/replies", app.base))
        .bearer_auth(&app.key)
        .json(&json!({"text":"tool answer"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.json::<Value>().await.unwrap()["delivery"]["outcome"],
        "confirmed"
    );
    assert!(
        app.tracker
            .drain(Instant::now() + Duration::from_secs(8))
            .await
    );
    app.drain().await;
    assert_eq!(app.fake.texts(), ["tool answer"]);
}
