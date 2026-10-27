use super::*;
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use cmaeg_core::seal::seal_blob;

impl Setup {
    fn http(&mut self) -> axum::Router {
        let channel = Arc::make_mut(&mut self.channel);
        channel.definition.record_id = uuid::Uuid::new_v4().to_string().parse().unwrap();
        channel.pointer.record_id = channel.definition.record_id.clone();
        channel.pointer.record_url = self
            .server
            .records
            .record_url(channel.definition.record_id.as_str())
            .unwrap()
            .to_string();
        channel.id = channel.pointer.seal_id(&fixture::keys()).unwrap();
        let blob = seal_blob(
            &fixture::keys(),
            &serde_json::to_vec(&channel.definition.to_blob_json()).unwrap(),
        )
        .unwrap();
        self.server.memory.channels.lock().unwrap().insert(
            channel.definition.record_id.to_string(),
            super::super::routing_store::test_server::Row {
                owner: channel.definition.owner.subject.clone(),
                blob,
                expiry: i64::MAX,
                version: 1,
            },
        );
        crate::http::whatsapp::router(self.state.clone())
    }
    fn callback(&self, body: Vec<u8>, valid: bool) -> Request<Body> {
        let tag = ring::hmac::sign(
            &ring::hmac::Key::new(
                ring::hmac::HMAC_SHA256,
                if valid {
                    credentials().app_secret.as_str().as_bytes().to_vec()
                } else {
                    b"wrong-secret".to_vec()
                }
                .as_slice(),
            ),
            &body,
        );
        Request::builder()
            .method("POST")
            .uri(format!("/v1/whatsapp/{}", self.channel.id))
            .header("content-type", "application/json")
            .header(
                "x-hub-signature-256",
                format!("sha256={}", webhook::hex(tag.as_ref())),
            )
            .body(Body::from(body))
            .unwrap()
    }
}
fn current(mut value: Value) -> Value {
    value["timestamp"] = time::OffsetDateTime::now_utc()
        .unix_timestamp()
        .to_string()
        .into();
    value
}
async fn json_response(response: axum::response::Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 8192).await.unwrap()).unwrap()
}

async fn call(app: axum::Router, request: Request<Body>) -> axum::response::Response {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let (parts, body) = request.into_parts();
    let response = reqwest::Client::new()
        .request(parts.method, format!("http://{address}{}", parts.uri))
        .headers(parts.headers)
        .body(to_bytes(body, 65536).await.unwrap())
        .send()
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.bytes().await.unwrap();
    task.abort();
    axum::response::Response::builder()
        .status(status)
        .body(Body::from(bytes))
        .unwrap()
}

#[tokio::test]
async fn native_whatsapp_http_challenge_authenticates_exact_token_and_rejects_duplicates() {
    let mut setup = Setup::new().await;
    let app = setup.http();
    for (query, status) in [
        ("hub.mode=subscribe&hub.verify_token=fixture-verify-token-12345&hub.challenge=8675309", StatusCode::OK),
        ("hub.mode=subscribe&hub.verify_token=wrong&hub.challenge=8675309", StatusCode::UNAUTHORIZED),
        ("hub.mode=subscribe&hub.verify_token=fixture-verify-token-12345&hub.verify_token=wrong&hub.challenge=8675309", StatusCode::UNAUTHORIZED),
    ] {
        let response = call(app.clone(), Request::builder().uri(format!("/v1/whatsapp/{}?{query}", setup.channel.id)).body(Body::empty()).unwrap()).await;
        assert_eq!(response.status(), status);
        if status == StatusCode::OK { assert_eq!(to_bytes(response.into_body(), 8192).await.unwrap(), "8675309"); }
    }
    assert_eq!(setup.server.memory.writes.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn native_whatsapp_http_authenticates_original_batch_before_durable_admission() {
    let mut setup = Setup::new().await;
    let app = setup.http();
    // Reaction precedes its original in the Meta batch; text authority commits first.
    let body = envelope(vec![
        current(reaction("wamid.http-reaction", "wamid.http", "👍")),
        current(text("wamid.http", "hello")),
    ]);
    let response = call(app.clone(), setup.callback(body.clone(), false)).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(setup.server.memory.writes.load(Ordering::SeqCst), 0);
    let response = call(app.clone(), setup.callback(body.clone(), true)).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        json_response(response).await,
        json!({"accepted":2,"refused":0})
    );
    let before = setup.server.memory.writes.load(Ordering::SeqCst);
    let response = call(app, setup.callback(body, true)).await;
    assert_eq!(
        json_response(response).await,
        json!({"accepted":2,"refused":0})
    );
    assert_eq!(setup.server.memory.writes.load(Ordering::SeqCst), before);
    assert!(setup
        .server
        .store()
        .load(&setup.identity("wamid.http-reaction"))
        .await
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn native_whatsapp_http_signed_omitted_emoji_sample_maps_to_removal_and_replays() {
    let mut setup = Setup::new().await;
    let app = setup.http();
    let original = envelope(vec![current(text("wamid.original", "hello"))]);
    let response = call(app.clone(), setup.callback(original, true)).await;
    assert_eq!(response.status(), StatusCode::OK);
    // Meta's documented full removal envelope, with fixture account/sender IDs
    // and a current timestamp for authenticated HTTP admission.
    // https://developers.facebook.com/documentation/business-messaging/whatsapp/webhooks/reference/messages/reaction
    let body = serde_json::to_vec(&json!({
        "object": "whatsapp_business_account",
        "entry": [{
            "id": "23456",
            "changes": [{
                "value": {
                    "messaging_product": "whatsapp",
                    "metadata": {
                        "display_phone_number": "15550783881",
                        "phone_number_id": "34567"
                    },
                    "contacts": [{
                        "profile": {"name": "Sheena Nelson"},
                        "wa_id": "447700900123"
                    }],
                    "messages": [current(json!({
                        "from": "447700900123",
                        "id": "wamid.removal-sample",
                        "type": "reaction",
                        "reaction": {"message_id": "wamid.original"}
                    }))]
                },
                "field": "messages"
            }]
        }]
    }))
    .unwrap();
    let before = setup.server.memory.writes.load(Ordering::SeqCst);
    let response = call(app.clone(), setup.callback(body.clone(), false)).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(setup.server.memory.writes.load(Ordering::SeqCst), before);
    let response = call(app.clone(), setup.callback(body.clone(), true)).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        json_response(response).await,
        json!({"accepted":1,"refused":0})
    );
    let plan = setup
        .server
        .store()
        .load(&setup.identity("wamid.removal-sample"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(plan.source_body, body);
    assert_eq!(plan.activity.actor.id, "447700900123");
    let ActivityKind::Event { ce, .. } = &plan.activity.kind else {
        panic!()
    };
    assert_eq!(ce.r#type, "cma.message.reaction.updated.v1");
    assert_eq!(ce.subject.as_deref(), Some("wamid.original"));
    let data = ce.data.as_ref().unwrap();
    assert_eq!(data["new_reactions"], json!([]));
    assert!(data.get("old_reactions").is_none());
    let before = setup.server.memory.writes.load(Ordering::SeqCst);
    let response = call(app, setup.callback(body, true)).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        json_response(response).await,
        json!({"accepted":1,"refused":0})
    );
    assert_eq!(setup.server.memory.writes.load(Ordering::SeqCst), before);
}

#[tokio::test]
async fn native_whatsapp_http_unknown_targets_and_wrong_phone_never_launch() {
    let mut setup = Setup::new().await;
    let app = setup.http();
    let recent = current(reaction("wamid.unknown-http", "wamid.absent", ""));
    let response = call(
        app.clone(),
        setup.callback(envelope(vec![recent.clone()]), true),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let mut expired = recent;
    expired["timestamp"] = (time::OffsetDateTime::now_utc().unix_timestamp() - 121)
        .to_string()
        .into();
    let body = envelope(vec![expired]);
    let response = call(app.clone(), setup.callback(body, true)).await;
    assert_eq!(
        json_response(response).await,
        json!({"accepted":0,"refused":1})
    );
    let mut body: Value =
        serde_json::from_slice(&envelope(vec![current(text("wamid.wrong-phone", "hello"))]))
            .unwrap();
    body["entry"][0]["changes"][0]["value"]["metadata"]["phone_number_id"] = "99999".into();
    let response = call(
        app,
        setup.callback(serde_json::to_vec(&body).unwrap(), true),
    )
    .await;
    assert_eq!(
        json_response(response).await,
        json!({"accepted":0,"refused":0})
    );
    assert_eq!(setup.server.memory.writes.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn native_whatsapp_http_media_authenticates_original_bytes_before_retaining_each_type() {
    let mut setup = Setup::new().await;
    setup.state = setup
        .state
        .with_media_enabled(true)
        .with_whatsapp_media(true);
    let app = setup.http();
    let messages = ["image", "document", "audio", "video", "sticker"]
        .iter()
        .map(|kind| {
            current(crate::whatsapp::media_tests::message(
                kind,
                &format!("wamid.{kind}"),
                b"fixture",
            ))
        })
        .collect();
    let body = envelope(messages);
    let response = call(app.clone(), setup.callback(body.clone(), false)).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(setup.server.memory.writes.load(Ordering::SeqCst), 0);
    let response = call(app.clone(), setup.callback(body.clone(), true)).await;
    assert_eq!(
        json_response(response).await,
        json!({"accepted":5,"refused":0})
    );
    for kind in ["image", "document", "audio", "video", "sticker"] {
        let plan = setup
            .server
            .store()
            .load(&setup.identity(&format!("wamid.{kind}")))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(plan.source_body, body);
        let ActivityKind::Message { attachments, .. } = plan.activity.kind else {
            panic!()
        };
        assert_eq!(attachments.len(), 1);
    }
    let writes = setup.server.memory.writes.load(Ordering::SeqCst);
    let response = call(app, setup.callback(body, true)).await;
    assert_eq!(
        json_response(response).await,
        json!({"accepted":5,"refused":0})
    );
    assert_eq!(setup.server.memory.writes.load(Ordering::SeqCst), writes);
}
