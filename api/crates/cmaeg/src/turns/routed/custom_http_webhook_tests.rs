use super::{custom_http_fixture::App, Phase, TargetState};
use crate::{
    auth::webhook,
    delivery::webhook_client::{TransportError, WebhookTransport},
};
use cmaeg_core::channel::Secret;
use futures_util::future::BoxFuture;
use reqwest::{header::HeaderMap, Url};
use serde_json::{json, Value};
use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(Default)]
struct Receiver(Mutex<Vec<Value>>);
impl WebhookTransport for Receiver {
    fn resolve(&self, _: Url) -> BoxFuture<'_, Result<Vec<SocketAddr>, TransportError>> {
        Box::pin(async { Ok(vec!["8.8.8.8:443".parse().unwrap()]) })
    }
    fn post(
        &self,
        url: Url,
        _: Vec<SocketAddr>,
        headers: HeaderMap,
        body: Arc<Vec<u8>>,
    ) -> BoxFuture<'_, Result<u16, TransportError>> {
        assert_eq!(url.as_str(), "https://receiver.example/events");
        let signature = webhook::signature(
            &Secret::new("Zml4dHVyZQ=="),
            headers["webhook-id"].to_str().unwrap(),
            headers["webhook-timestamp"].to_str().unwrap(),
            &body,
        );
        assert_eq!(headers["webhook-signature"], signature);
        self.0
            .lock()
            .unwrap()
            .push(serde_json::from_slice(&body).unwrap());
        Box::pin(async { Ok(200) })
    }
}
async fn app(auto_final: bool, receiver: Arc<Receiver>) -> App {
    App::with_delivery(auto_final, |value| {
        value["reply"]["sinks"] = json!([{"type":"result_webhook","url":"https://receiver.example/events","hmac_key_id":"key-1"}]);
    }, None, Some(receiver)).await
}
async fn settled(app: &App) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let plans = app.plans().await;
            if let Some(plan) = plans.first() {
                if let Some((state, _)) = app
                    .journal
                    .read::<TargetState>(&plan.targets[0].id)
                    .await
                    .unwrap()
                {
                    if matches!(state.phase, Phase::Completed { .. }) {
                        break;
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn webhook_only_custom_completion_is_signed_and_independent_of_auto_final() {
    for auto_final in [true, false] {
        let receiver = Arc::new(Receiver::default());
        let mut app = app(auto_final, receiver.clone()).await;
        let response = app
            .request(app.event("async-completion").to_string(), false)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 202);
        settled(&app).await;
        app.drain().await;
        let events = receiver.0.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["type"], "cmaeg.response.completed");
        assert_eq!(events[0]["subject"], "async-completion");
        assert_eq!(
            events[0]["data"]["text"],
            if auto_final {
                json!("hello world")
            } else {
                Value::Null
            }
        );
        assert!(app.waiters.is_empty());
    }
}

#[tokio::test]
async fn webhook_only_custom_failure_sends_failure_lifecycle_instead_of_success_message() {
    let receiver = Arc::new(Receiver::default());
    let mut app = app(false, receiver.clone()).await;
    let mut frames = super::executor_tests::frames();
    frames[3] = json!({"type":"response.failed","sequence_number":3,
        "response":{"id":"resp_accepted","object":"response","status":"failed",
        "conversation":{"id":"agt_native"},"metadata":{},"output":[],
        "error":{"code":"server_error","message":"fixture failure"}}});
    app.provider.memory.lock().unwrap().config.replay_frames = Some(frames);
    assert_eq!(
        app.request(app.event("async-failure").to_string(), false)
            .send()
            .await
            .unwrap()
            .status(),
        202
    );
    settled(&app).await;
    app.drain().await;
    let events = receiver.0.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["type"], "cmaeg.response.failed");
    assert!(!events[0]["data"]["error"].is_null());
}

#[tokio::test]
async fn webhook_only_custom_message_commit_and_completion_are_distinct_signed_callbacks() {
    let receiver = Arc::new(Receiver::default());
    let mut app = app(true, receiver.clone()).await;
    let mut frames = super::executor_tests::frames();
    frames.insert(3, json!({"type":"response.output_text.done","sequence_number":3,"item_id":"m","text":"hello world"}));
    frames[4]["sequence_number"] = 4.into();
    app.provider.memory.lock().unwrap().config.replay_frames = Some(frames);
    assert_eq!(
        app.request(app.event("async-message").to_string(), false)
            .send()
            .await
            .unwrap()
            .status(),
        202
    );
    settled(&app).await;
    app.drain().await;
    let events = receiver.0.lock().unwrap();
    assert_eq!(
        events
            .iter()
            .map(|event| event["type"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["cmaeg.response.message", "cmaeg.response.completed"]
    );
    assert_ne!(events[0]["id"], events[1]["id"]);
}
