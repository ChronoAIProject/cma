//! The real delivery worker must refuse unavailable explicit destinations.
use super::support::App;
use cmaeg::DeliveryConfig;
use serde_json::{json, Value};

#[tokio::test]
async fn a_legacy_channel_never_sends_an_explicit_default_to_its_origin() {
    let mut app = App::new(
        vec![],
        0,
        DeliveryConfig {
            multi_consumer_enabled: true,
            ..Default::default()
        },
        Default::default(),
    )
    .await;
    let response = app
        .fixture
        .client
        .post(format!(
            "{}/v1/events/{}/replies?destination=default",
            app.base,
            app.turn.event_ref.as_str()
        ))
        .bearer_auth(&app.key)
        .json(&json!({"text":"private destination"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let receipt: Value = response.json().await.unwrap();
    assert_eq!(
        receipt["delivery"],
        json!({"outcome":"rejected","reason":"default_reply_unavailable"})
    );
    app.drain().await;
    assert!(app.fake.requests.lock().unwrap().is_empty());
}
