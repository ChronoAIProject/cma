use super::support::App;
use cmaeg::{
    auth::AgentCaller,
    delivery::{
        custom::CustomDelivery,
        engine::DeliveryEngine,
        intents::DeliveryOutcome,
        relay_client::{RelayClient, RelayTransport},
        router::ShardedScheduler,
    },
    net::exchange::{HttpReply, TransportError},
    DeliveryConfig,
};
use futures_util::future::BoxFuture;
use reqwest::{header::HeaderMap, StatusCode, Url};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{sync::mpsc, time::Instant};

struct DelayedTransport {
    delay: Duration,
    calls: Mutex<Vec<Duration>>,
}
impl RelayTransport for DelayedTransport {
    fn post(
        &self,
        _: Url,
        _: HeaderMap,
        _: Vec<u8>,
        timeout: Duration,
    ) -> BoxFuture<'_, Result<HttpReply, TransportError>> {
        self.calls.lock().unwrap().push(timeout);
        Box::pin(async move {
            tokio::time::sleep(self.delay).await;
            Ok(HttpReply::new(
                StatusCode::OK,
                br#"{"message_id":"receipt-1","platform_message_id":"wamid.sent"}"#.to_vec(),
            ))
        })
    }
}

#[tokio::test]
async fn media_template_tool_observes_the_real_engine_receipt_and_transport_deadline() {
    for (kind, delay, expected_elapsed, confirmed) in [
        ("image", 12, 12, true),
        ("video", 12, 12, true),
        ("document", 12, 12, true),
        ("image", 61, 60, false),
        ("text", 6, 5, false),
    ] {
        let mut app = App::on_private("whatsapp").await;
        app.state.whatsapp_templates_enabled = true;
        let caller = AgentCaller {
            key_hash: Sha256::digest(app.key.as_bytes()).into(),
        };
        // Cache actual authority before pausing time: the reply rechecks this
        // authority while the real delivery worker and relay use virtual time.
        app.state
            .authorize_event(caller, &app.event_ref, "replyToEvent")
            .await
            .unwrap();
        let transport = Arc::new(DelayedTransport {
            delay: Duration::from_secs(delay),
            calls: Mutex::new(vec![]),
        });
        let config = DeliveryConfig::default();
        let relay = RelayClient::new("https://relay.example", config.reply_timeout)
            .unwrap()
            .with_whatsapp_templates(true)
            .with_transport(transport.clone());
        let (sender, receiver) = mpsc::channel(8);
        app.state.delivery = Arc::new(ShardedScheduler::new(sender));
        let engine = DeliveryEngine::start(
            receiver,
            app.state.keys.clone(),
            relay,
            config,
            app.state.tracker.clone(),
            CustomDelivery::new("https://gateway.example".into(), Arc::default()),
        );
        let components = if kind == "text" {
            json!([])
        } else {
            json!([{"type":"header","parameters":[{"type":kind,kind:{"id":"123456789"}}]}])
        };
        let message = serde_json::from_value(json!({"whatsapp_template":{
            "name":"invoice","language":{"code":"en_US"},"components":components
        }}))
        .unwrap();
        tokio::time::pause();
        let started = Instant::now();
        let result = app
            .state
            .reply(caller, &app.event_ref, message)
            .await
            .unwrap();
        let elapsed = started.elapsed();
        let expected = Duration::from_secs(expected_elapsed);
        assert!(
            (expected..=expected + Duration::from_millis(2)).contains(&elapsed),
            "{kind}: {elapsed:?}"
        );
        if confirmed {
            assert!(matches!(result.delivery, DeliveryOutcome::Confirmed { .. }));
            assert!(result.message_ref.is_some());
        } else {
            assert!(matches!(result.delivery, DeliveryOutcome::Unknown { .. }));
            assert!(result.message_ref.is_none());
        }
        // A transport timeout is uncertain; the engine must not resend it.
        assert_eq!(
            *transport.calls.lock().unwrap(),
            [Duration::from_secs(if kind == "text" { 5 } else { 60 })]
        );
        drop(engine);
        drop(app);
        tokio::time::resume();
    }
}
