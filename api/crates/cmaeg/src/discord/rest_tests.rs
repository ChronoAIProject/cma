use super::{
    rest::{PreparedFile, RestError},
    rest_fixture::Fixture,
};
use axum::http::StatusCode;
use cmaeg_core::{channel::Secret, message::NativeAttachment};
use serde_json::{json, Value};

#[tokio::test]
async fn native_text_uses_exact_bot_custody_and_never_mentions() {
    let f = Fixture::new().await;
    let token = Secret::new("native-local-fixture");
    assert_eq!(
        f.api
            .send(&token, "77", "99", Some("700"), "@everyone <@123>", &[])
            .await,
        Ok("800".into())
    );
    let calls = f.memory.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].path, "/api/v10/channels/99/messages");
    assert_eq!(
        calls[0].authorization.as_deref(),
        Some("Bot native-local-fixture")
    );
    let body: Value = serde_json::from_slice(&calls[0].body).unwrap();
    assert_eq!(
        body["allowed_mentions"],
        json!({"parse":[],"replied_user":false})
    );
    assert_eq!(body["message_reference"]["message_id"], "700");
    assert!(!String::from_utf8_lossy(&calls[0].body).contains("native-local-fixture"));
}
#[tokio::test]
async fn native_multipart_preserves_bytes_names_captions_and_text() {
    let f = Fixture::new().await;
    let file = PreparedFile {
        bytes: vec![1, 2, 3, 4],
        filename: "diagram.png".into(),
        mime: "image/png".into(),
        caption: Some("diagram".into()),
    };
    f.api
        .send(
            &Secret::new("native-local-fixture"),
            "77",
            "99",
            None,
            "file attached",
            &[file],
        )
        .await
        .unwrap();
    let calls = f.memory.calls.lock().unwrap();
    assert!(calls[0]
        .content_type
        .as_ref()
        .unwrap()
        .starts_with("multipart/form-data; boundary="));
    let body = String::from_utf8_lossy(&calls[0].body);
    assert!(body.contains("name=\"files[0]\"; filename=\"diagram.png\""));
    assert!(body.contains("\"description\":\"diagram\""));
    assert!(body.contains("file attached"));
    assert!(calls[0].body.windows(4).any(|bytes| bytes == [1, 2, 3, 4]));
}
#[tokio::test]
async fn rate_limit_blocks_following_requests_without_retrying_a_write() {
    let f = Fixture::new().await;
    f.respond(
        StatusCode::TOO_MANY_REQUESTS,
        json!({"retry_after":2.25,"global":true}),
    );
    let token = Secret::new("native-local-fixture");
    assert_eq!(
        f.api.send(&token, "77", "99", None, "hello", &[]).await,
        Err(RestError::RateLimited(3))
    );
    assert!(matches!(
        f.api.send(&token, "77", "100", None, "hello", &[]).await,
        Err(RestError::RateLimited(_))
    ));
    assert_eq!(f.memory.calls.lock().unwrap().len(), 1);
}
#[tokio::test]
async fn response_loss_server_failure_and_unbound_success_are_unknown_not_retried() {
    let f = Fixture::new().await;
    let token = Secret::new("native-local-fixture");
    *f.memory.broken.lock().unwrap() = true;
    assert_eq!(
        f.api.send(&token, "77", "99", None, "hello", &[]).await,
        Err(RestError::Unknown)
    );
    *f.memory.broken.lock().unwrap() = false;
    f.respond(StatusCode::BAD_GATEWAY, json!({}));
    assert_eq!(
        f.api.send(&token, "77", "99", None, "hello", &[]).await,
        Err(RestError::Unknown)
    );
    f.respond(
        StatusCode::OK,
        json!({"id":"800","channel_id":"other","author":{"id":"77"}}),
    );
    assert_eq!(
        f.api.send(&token, "77", "99", None, "hello", &[]).await,
        Err(RestError::Unknown)
    );
    assert_eq!(f.memory.calls.lock().unwrap().len(), 3);
}
#[tokio::test]
async fn redirect_and_invalid_ids_are_refused_and_metadata_is_exact() {
    let f = Fixture::new().await;
    let token = Secret::new("native-local-fixture");
    assert_eq!(
        f.api
            .send(&token, "77", "../users", None, "hello", &[])
            .await,
        Err(RestError::Rejected)
    );
    assert!(f.memory.calls.lock().unwrap().is_empty());
    f.respond(StatusCode::TEMPORARY_REDIRECT, json!({}));
    assert_eq!(
        f.api.send(&token, "77", "99", None, "hello", &[]).await,
        Err(RestError::Rejected)
    );
    f.respond(
        StatusCode::OK,
        json!({"id":"701","channel_id":"99","attachments":[]}),
    );
    assert_eq!(
        f.api.message(&token, "99", "700").await,
        Err(RestError::Rejected)
    );
    assert_eq!(f.memory.calls.lock().unwrap().len(), 2);
}
#[tokio::test]
async fn outbound_inline_media_validation_preserves_bytes_and_refuses_unsafe_names() {
    let item: NativeAttachment = serde_json::from_value(json!({"kind":"image","source":{"type":"base64","data":"AQIDBA=="},"filename":"image.png","mime_type":"image/png"})).unwrap();
    let files = super::outbound_media::prepare(std::slice::from_ref(&item))
        .await
        .unwrap();
    assert_eq!(files[0].bytes, [1, 2, 3, 4]);
    let mut unsafe_item = item;
    unsafe_item.filename = Some("../image.png".into());
    assert!(super::outbound_media::prepare(&[unsafe_item])
        .await
        .is_err());
}
