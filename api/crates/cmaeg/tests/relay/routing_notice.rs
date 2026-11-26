use super::support::*;
use cmaeg::{
    delivery::relay_client::{RelayClient, RelayTransport},
    net::exchange::{HttpReply, TransportError},
    turns::observer::ActivityObserver,
};
use cmaeg_core::ids::{EventId, RecordId};
use futures_util::future::BoxFuture;
use reqwest::{header::HeaderMap, StatusCode, Url};
use serde_json::{json, Value};
use std::{
    sync::{atomic::Ordering, Arc, Mutex},
    time::Duration,
};

#[derive(Default)]
struct ObservedRefusals(Mutex<Vec<(String, String)>>);

impl ActivityObserver for ObservedRefusals {
    fn refused(&self, _: &RecordId, source: &'static str, event: Option<&EventId>, reason: &str) {
        assert_eq!(source, "nyxid_relay");
        self.0
            .lock()
            .unwrap()
            .push((event.unwrap().to_string(), reason.into()));
    }
}

#[derive(Default)]
struct ReplyRecorder(Mutex<Vec<(String, Vec<u8>, Value)>>);

impl RelayTransport for ReplyRecorder {
    fn post(
        &self,
        url: Url,
        headers: HeaderMap,
        body: Vec<u8>,
        _: Duration,
    ) -> BoxFuture<'_, Result<HttpReply, TransportError>> {
        Box::pin(async move {
            self.0.lock().unwrap().push((
                url.path().into(),
                headers["authorization"].as_bytes().to_vec(),
                serde_json::from_slice(&body).unwrap(),
            ));
            Ok(HttpReply {
                status: StatusCode::OK,
                headers: HeaderMap::new(),
                body: Ok(br#"{"message_id":"reply-fixture","platform_message_id":"12"}"#.to_vec()),
            })
        })
    }
}

#[tokio::test]
async fn unknown_telegram_targets_are_refused_and_explained_only_when_enabled() {
    for explain in [true, false] {
        let replies = Arc::new(ReplyRecorder::default());
        let refusals = Arc::new(ObservedRefusals::default());
        let relay = RelayClient::new("https://nyx.example", Duration::from_secs(1))
            .unwrap()
            .with_transport(replies.clone());
        let fixture = Fixture::with_ingress(
            |definition| {
                definition["routing"] = json!({
                    "version": 1,
                    "default_consumer": {"name": "default"},
                    "default_targets": ["default"],
                    "addressing": {"command": true, "unknown_target_notice": explain}
                });
            },
            |state| {
                state
                    .with_routing_notice_relay(Some(relay))
                    .with_activity_observer(refusals.clone())
            },
        )
        .await;
        fixture.recorder.outcome.store(1, Ordering::SeqCst);

        let mut expected_notices: Vec<(String, Vec<u8>, Value)> = Vec::new();
        for (number, text) in [
            (1, "/to missing-one hello"),
            (2, "/to missing-two hello"),
            (3, "/to default hello"),
            (4, "ordinary recovery"),
        ] {
            let message_id = format!("message-{number}");
            let mut envelope = Envelope::new();
            envelope.payload["message_id"] = json!(message_id);
            envelope.payload["content"]["text"] = json!(text);
            envelope.payload["raw_platform_data"]["message"]["text"] = json!(text);
            envelope.callback["message_id"] = json!(message_id);
            envelope.reply["inbound_message_id"] = json!(message_id);
            let (body, token) = envelope.signed();
            let payload: Value = serde_json::from_slice(&body).unwrap();
            let mut request = fixture.request(body, &token).build().unwrap();
            request
                .headers_mut()
                .insert("x-nyxid-message-id", message_id.parse().unwrap());
            let response = fixture.client.execute(request).await.unwrap();
            if number <= 2 {
                problem(response, 403, "target_unknown").await;
                assert!(fixture.recorder.requests.lock().unwrap().is_empty());
                if explain {
                    let reply_token = payload["reply_token"].as_str().unwrap();
                    expected_notices.push((
                        "/api/v1/channel-relay/reply".into(),
                        format!("Bearer {reply_token}").into_bytes(),
                        json!({
                            "message_id": message_id,
                            "reply": {"text": "Unknown target. Check the Consumer name and try again."}
                        }),
                    ));
                }
            } else {
                assert_eq!(response.status(), StatusCode::ACCEPTED);
            }
        }
        let launches = fixture.recorder.requests.lock().unwrap();
        assert_eq!(launches.len(), 2);
        for (request, id) in launches.iter().zip(["message-3", "message-4"]) {
            assert_eq!(request.activity.event_id.as_str(), id);
            assert!(request.consumer.is_none());
        }
        assert_eq!(
            *refusals.0.lock().unwrap(),
            [
                ("message-1".into(), "target_unknown".into()),
                ("message-2".into(), "target_unknown".into()),
            ]
        );
        let replies = replies.0.lock().unwrap();
        assert_eq!(replies.len(), expected_notices.len());
        for (actual, expected) in replies.iter().zip(expected_notices) {
            assert_eq!(actual.0, expected.0);
            assert_eq!(actual.1, expected.1);
            assert_eq!(actual.2["message_id"], expected.2["message_id"]);
            assert_eq!(actual.2["reply"]["text"], expected.2["reply"]["text"]);
        }
    }
}
