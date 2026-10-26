use super::{custom_http_fixture::App, Phase, TargetState};
use serde_json::Value;
use std::time::Duration;

async fn accepted(response: reqwest::Response) -> Value {
    assert_eq!(response.status(), 202);
    assert_eq!(response.headers()["content-type"], "application/json");
    response.json().await.unwrap()
}

async fn completed(app: &App) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let plans = app.plans().await;
            if let Some(plan) = plans.first() {
                if app
                    .journal
                    .read::<TargetState>(&plan.targets[0].id)
                    .await
                    .unwrap()
                    .is_some_and(|(state, _)| matches!(state.phase, Phase::Completed { .. }))
                {
                    return;
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the original admitted turn must finish");
}

async fn assert_one_turn(app: &App) {
    assert_eq!(app.plans().await.len(), 1);
    assert_eq!(
        app.provider
            .seen()
            .iter()
            .filter(|request| request.method == "POST")
            .count(),
        1
    );
    assert!(app.waiters.is_empty());
}

#[tokio::test]
async fn duplicate_custom_stream_returns_accepted_without_replacing_original_stream() {
    let mut app = App::new(true).await;
    let body = app.event("duplicate-stream").to_string();
    let original = app.request(body.clone(), true).send().await.unwrap();
    assert_eq!(original.status(), 200);
    let retry = tokio::time::timeout(
        Duration::from_secs(2),
        app.request(body.clone(), true).send(),
    )
    .await
    .expect("a duplicate must not await an unattached stream")
    .unwrap();
    let first_acceptance = accepted(retry).await;
    let stream = original.text().await.unwrap();
    assert!(stream.contains("hello "), "{stream}");
    assert!(stream.contains("world"), "{stream}");
    assert_eq!(stream.matches("event: cmaeg.completed").count(), 1);
    completed(&app).await;

    let completed_retry = app.request(body, true).send().await.unwrap();
    let second_acceptance = accepted(completed_retry).await;
    assert_eq!(
        first_acceptance["conversation_id"],
        second_acceptance["conversation_id"]
    );
    app.drain().await;
    assert_one_turn(&app).await;
}

#[tokio::test]
async fn asynchronous_custom_event_cannot_acquire_a_stream_on_retry() {
    let mut app = App::new(true).await;
    let body = app.event("async-to-stream").to_string();
    let initial = accepted(app.request(body.clone(), false).send().await.unwrap()).await;
    let retry = accepted(app.request(body, true).send().await.unwrap()).await;
    assert_eq!(initial["conversation_id"], retry["conversation_id"]);
    assert!(app.waiters.is_empty());
    completed(&app).await;
    app.drain().await;
    assert_one_turn(&app).await;
}

#[tokio::test]
async fn concurrent_custom_stream_duplicates_keep_only_the_winning_waiter() {
    let mut app = App::new(true).await;
    let body = app.event("concurrent-stream").to_string();
    let (left, right) = tokio::join!(
        app.request(body.clone(), true).send(),
        app.request(body, true).send()
    );
    let (left, right) = (left.unwrap(), right.unwrap());
    let (original, retry) = if left.status() == 200 {
        (left, right)
    } else {
        (right, left)
    };
    assert_eq!(original.status(), 200);
    accepted(retry).await;
    let stream = original.text().await.unwrap();
    assert_eq!(stream.matches("event: cmaeg.completed").count(), 1);
    completed(&app).await;
    app.drain().await;
    assert_one_turn(&app).await;
}

#[tokio::test]
async fn custom_stream_duplicate_with_changed_payload_conflicts_and_drops_new_waiter() {
    let mut app = App::new(true).await;
    let mut event = app.event("changed-stream");
    let original = app.request(event.to_string(), true).send().await.unwrap();
    assert_eq!(original.status(), 200);
    event["data"]["result"] = "changed".into();
    let conflict = app.request(event.to_string(), true).send().await.unwrap();
    assert_eq!(conflict.status(), 409);
    let stream = original.text().await.unwrap();
    assert_eq!(stream.matches("event: cmaeg.completed").count(), 1);
    completed(&app).await;
    app.drain().await;
    assert_one_turn(&app).await;
}
