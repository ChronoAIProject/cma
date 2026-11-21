//! Telegram retries a failed locator write without repeating the accepted agent turn.
use super::{custom_http_fixture::App, Phase, TargetState};
use serde_json::{json, Value};
use std::time::Duration;

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
    .unwrap();
}
async fn send(app: &App, raw: &Value) -> (u16, Value) {
    let response = reqwest::Client::new()
        .post(&app.url)
        .header("x-telegram-bot-api-secret-token", "fixture-webhook")
        .json(raw)
        .send()
        .await
        .unwrap();
    (response.status().as_u16(), response.json().await.unwrap())
}
#[tokio::test]
async fn telegram_locator_failure_retries_original_http_after_provider_completion_without_second_post(
) {
    for lost_ack in [false, true] {
        let mut app = App::configured(false, |value| {
            value["sources"] = json!([{"type":"telegram","bot_id":"77","message_events":true,"admission":{"type":"all"}}]);
            value["partition"]["key"] = "conversation_and_sender".into();
            value["reply"]["sinks"] = json!([{"type":"source"}]);
        }, None).await;
        let record = app
            .records
            .memory
            .channels
            .lock()
            .unwrap()
            .keys()
            .next()
            .unwrap()
            .clone();
        let context = crate::connections::store::index_id(
            "telegram-message-context-v1",
            &[&record, "77", "42", "5"],
        );
        app.records.fault(context.clone(), lost_ack, false);
        let now = time::OffsetDateTime::now_utc().unix_timestamp();
        let raw = json!({"update_id":101,"message":{"message_id":5,"date":now-5,
            "chat":{"id":42,"type":"private"},"from":{"id":42,"is_bot":false,"first_name":"Alice"},"text":"original"}});
        let (status, body) = send(&app, &raw).await;
        assert_eq!(status, 503, "{body}");
        completed(&app).await;
        let original = app.plans().await;
        assert_eq!(original.len(), 1);
        assert_eq!(
            app.provider
                .seen()
                .iter()
                .filter(|request| request.method == "POST")
                .count(),
            1
        );
        assert_eq!(
            app.records
                .memory
                .rows
                .lock()
                .unwrap()
                .contains_key(&context),
            lost_ack
        );
        let (status, body) = send(&app, &raw).await;
        assert_eq!(status, 202, "{body}");
        assert_eq!(body["status"], "accepted");
        assert_eq!(app.plans().await, original);
        assert!(app
            .records
            .memory
            .rows
            .lock()
            .unwrap()
            .contains_key(&context));
        app.drain().await;
        assert_eq!(
            app.provider
                .seen()
                .iter()
                .filter(|request| request.method == "POST")
                .count(),
            1
        );
    }
}
