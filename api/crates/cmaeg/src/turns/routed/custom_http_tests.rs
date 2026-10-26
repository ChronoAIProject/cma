use super::{custom_http_fixture::App, Phase, TargetState};

#[tokio::test]
async fn public_custom_http_stream_runs_real_routed_provider_and_closes_on_completion() {
    let mut app = App::new(true).await;
    let body = app.event("http-stream").to_string();
    let response = app.request(body.clone(), true).send().await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["content-type"], "text/event-stream");
    let stream = response.text().await.unwrap();
    assert!(stream.contains("event: cmaeg.text.delta"), "{stream}");
    assert!(stream.contains("hello "), "{stream}");
    assert!(stream.contains("world"), "{stream}");
    assert_eq!(
        stream.matches("event: cmaeg.completed").count(),
        1,
        "{stream}"
    );
    assert!(!stream.contains("cmaeg.failed"), "{stream}");
    app.drain().await;
    assert!(app.waiters.is_empty());
    let plans = app.plans().await;
    assert_eq!(plans.len(), 1);
    assert_eq!(plans[0].source_body, body.as_bytes());
    let state = app
        .journal
        .read::<TargetState>(&plans[0].targets[0].id)
        .await
        .unwrap()
        .unwrap()
        .0;
    assert!(matches!(state.phase, Phase::Completed { .. }));
    let seen = app.provider.seen();
    assert_eq!(
        seen.iter()
            .filter(|request| request.method == "POST")
            .count(),
        1
    );
    let post: serde_json::Value = serde_json::from_slice(
        &seen
            .iter()
            .find(|request| request.method == "POST")
            .unwrap()
            .body,
    )
    .unwrap();
    assert_eq!(
        post["event_context"]["activity"]["source"]["type"],
        "custom"
    );
    assert!(post["input"].to_string().contains("passed"));
    let activity = app
        .activity
        .latest(
            &plans[0].identity.owner,
            plans[0].identity.record_id.as_str(),
        )
        .await
        .unwrap();
    assert_eq!(activity.items.len(), 1);
    assert_eq!(
        activity.items[0].event_id,
        plans[0].identity.event_id.as_str()
    );
    let consumer = &activity.items[0].consumers[0];
    assert_eq!(consumer.state, crate::activity::ConsumerState::Completed);
    assert!(
        consumer.dispatched_at.is_some()
            && consumer.accepted_at.is_some()
            && consumer.produced_at.is_some()
            && consumer.completed_at.is_some()
    );
    assert!(consumer.agent_id.is_some() && consumer.response_id.is_some());
    assert!(consumer
        .deliveries
        .iter()
        .any(|d| d.outcome == "buffered" && d.confirmed_at.is_none()));
}

#[tokio::test]
async fn public_custom_stream_closes_even_when_auto_final_is_disabled() {
    let mut app = App::new(false).await;
    let response = app
        .request(app.event("http-no-final").to_string(), true)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let stream = response.text().await.unwrap();
    assert_eq!(
        stream.matches("event: cmaeg.completed").count(),
        1,
        "{stream}"
    );
    app.drain().await;
    assert!(app.waiters.is_empty());
}

#[tokio::test]
async fn public_custom_authentication_failure_does_not_admit_or_contact_provider() {
    let mut app = App::new(true).await;
    let response = reqwest::Client::new()
        .post(&app.url)
        .header("content-type", "application/cloudevents+json")
        .body(app.event("unauthenticated").to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 401);
    app.drain().await;
    assert!(app.plans().await.is_empty());
    assert!(app.records.memory.rows.lock().unwrap().is_empty());
    assert!(app.provider.seen().is_empty());
    assert!(app.waiters.is_empty());
}
