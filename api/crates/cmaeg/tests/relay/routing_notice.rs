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
struct ReplyRecorder(Mutex<Vec<(String, Vec<u8>, Value)>>, bool);

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
            if self.1 {
                return Err(TransportError::Unavailable);
            }
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
        let page = owner_activity(&fixture).await;
        let refusals_recorded: Vec<_> = page["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|item| item["source_refusal"] == "target_unknown")
            .collect();
        assert_eq!(refusals_recorded.len(), 2);
        assert!(refusals_recorded.iter().all(|item| {
            item["consumers"] == json!([])
                && item["actor"]["id"] == "redacted"
                && item["conversation"]["id"] == "redacted"
        }));
        assert!(!page.to_string().contains("missing-one"));
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

#[tokio::test]
async fn captioned_photo_media_off_explains_refusal_without_launching() {
    for uncertain in [false, true] {
        assert_media_notice(uncertain).await;
    }
}

async fn assert_media_notice(uncertain: bool) {
    let replies = Arc::new(ReplyRecorder(Mutex::new(Vec::new()), uncertain));
    let relay = RelayClient::new("https://nyx.example", Duration::from_secs(1))
        .unwrap()
        .with_transport(replies.clone());
    let fixture =
        Fixture::with_ingress(|_| {}, |state| state.with_routing_notice_relay(Some(relay))).await;
    let mut photo = super::nyxid_shapes::telegram_photo();
    fixture
        .backend
        .interaction_write_status
        .store(503, Ordering::SeqCst);
    assert_eq!(fixture.send(&photo).await.status(), 503);
    assert!(replies.0.lock().unwrap().is_empty());
    fixture
        .backend
        .interaction_write_status
        .store(200, Ordering::SeqCst);
    let response = fixture.send(&photo).await;
    assert_eq!(response.status(), 202);
    assert_eq!(
        response.json::<Value>().await.unwrap(),
        json!({"status": "refused", "reason": "attachment_unsupported"})
    );

    // A separate replica shares the durable owner store and notice claim.
    let activity = Arc::new(cmaeg::activity::ActivityStore::new(
        (*fixture.records).clone(),
        fixture.state.master_keys.clone(),
    ));
    let replica = fixture.state.clone().with_activity_observer(Arc::new(
        cmaeg::activity::ObservedActivity::new(activity, fixture.state.activity.clone()),
    ));
    let (base, replica_task) = serve(cmaeg::http::callbacks::router(replica)).await;
    photo.reply["jti"] = "renewed-reply-fixture".into();
    let (body, token) = photo.signed();
    let mut request = fixture.request(body, &token).build().unwrap();
    *request.url_mut() = format!("{base}/v1/callbacks/{}", fixture.id)
        .parse()
        .unwrap();
    assert_eq!(fixture.client.execute(request).await.unwrap().status(), 202);
    replica_task.abort();

    let page = owner_activity(&fixture).await;
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    assert_eq!(page["items"][0]["source_refusal"], "attachment_unsupported");
    assert_eq!(page["items"][0]["consumers"], json!([]));
    assert!(!page.to_string().contains("look at this"));
    assert!(fixture.recorder.requests.lock().unwrap().is_empty());
    let replies = replies.0.lock().unwrap();
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0].2["message_id"], "message-1");
    assert_eq!(
        replies[0].2["reply"]["text"],
        "Attachments are unavailable. Please send your message as text."
    );
}

async fn owner_activity(fixture: &Fixture) -> Value {
    let identity = crate::fake_nyxid::FakeNyxid::start().await;
    let (owner, _) = identity.oauth.issue_tokens("user-1");
    let manager = crate::management_fixture::state(
        Default::default(),
        fixture.records.clone(),
        fixture.state.master_keys.clone(),
        &fixture.record_origin,
        &identity.base_url,
    );
    let (base, task) = serve(cmaeg::http::management_router(manager)).await;
    let response = fixture
        .client
        .get(format!("{base}/v1/channels/{}/activity", fixture.id))
        .bearer_auth(owner)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let page = response.json().await.unwrap();
    task.abort();
    page
}

#[tokio::test]
async fn media_notice_requires_positive_photo_and_group_admission() {
    let replies = Arc::new(ReplyRecorder::default());
    let relay = RelayClient::new("https://nyx.example", Duration::from_secs(1))
        .unwrap()
        .with_transport(replies.clone());
    let fixture = Fixture::with_ingress(
        |definition| {
            definition["sources"][0]["admission"] = json!({
                "type": "scoped", "senders": {"type": "open"}, "chats": {"type": "open"},
                "groups": "mention"
            });
        },
        |state| state.with_routing_notice_relay(Some(relay)),
    )
    .await;
    let mut group_photo = super::nyxid_shapes::telegram_photo();
    group_photo.group();
    assert_eq!(fixture.send(&group_photo).await.status(), 202);
    let mut malformed = super::nyxid_shapes::telegram_photo();
    malformed.payload["raw_platform_data"]["message"]["photo"] = json!("invalid");
    assert_eq!(fixture.send(&malformed).await.status(), 202);
    assert!(replies.0.lock().unwrap().is_empty());
    assert!(fixture.recorder.requests.lock().unwrap().is_empty());
}
