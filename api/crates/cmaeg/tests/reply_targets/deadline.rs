//! Durable reply-target answers are identical before and after late completion.
use super::support::{receipt, App};
use serde_json::json;
use std::time::Duration;
use tokio::time::Instant;

#[tokio::test]
async fn target_deadline_stores_the_same_receipt_for_every_replay() {
    for routed in [false, true] {
        let mut app = App::new(vec![]).await;
        if routed {
            app.definition.routing = Some(
                serde_json::from_value(json!({
                "version":1,"default_consumer":{"name":"lead"},"consumers":[],"default_targets":["default"]
                }))
                .unwrap(),
            );
            app.update().await;
            app.state.multi_consumer_enabled = true;
        }
        let target = app.target(&app.event("alice", "late-target").await).await;
        let commands = app.hold_delivery().await;
        let started = Instant::now();
        let first = receipt(
            app.send(&target, "same", "send only once")
                .send()
                .await
                .unwrap(),
        )
        .await;
        assert!(started.elapsed() < Duration::from_millis(8500));
        assert_eq!(
            first["delivery"],
            json!({"outcome":"unknown","reason":"delivery_unconfirmed"})
        );
        assert!(app.relay.requests.lock().unwrap().is_empty());
        let replay = receipt(
            app.send(&target, "same", "send only once")
                .send()
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(first, replay);
        app.release_delivery(commands);
        app.drain().await;
        assert_eq!(
            app.relay.requests.lock().unwrap().len(),
            usize::from(routed)
        );
        let replay = receipt(
            app.send(&target, "same", "send only once")
                .send()
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(first, replay);
        assert_eq!(
            app.relay.requests.lock().unwrap().len(),
            usize::from(routed)
        );
    }
}
