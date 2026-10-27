//! Provider wire/receipt boundaries for caption and replacement edits.
use super::{
    media_edit,
    media_fixture::{Answer, Fixture},
    Error,
};
use cmaeg_core::{channel::Secret, message::NativeMediaKind};
use reqwest::StatusCode;
use serde_json::{json, Value};
fn document(caption: &str) -> Value {
    json!({"message_id":800,"chat":{"id":-42},"message_thread_id":7,
        "from":{"id":77,"is_bot":true},"caption":caption,
        "document":{"file_id":"locator","file_unique_id":"stable","file_name":"report.bin","mime_type":"application/octet-stream","file_size":4}})
}
fn token() -> Secret {
    Secret::new(format!("77:{}", uuid::Uuid::new_v4().simple()))
}
#[tokio::test]
async fn media_caption_uses_exact_message_without_thread_parameter_or_send_fallback() {
    let f = Fixture::new().await;
    f.answer(Answer::json(
        StatusCode::OK,
        json!({"ok":true,"result":document("")}),
    ));
    let edit = f
        .api
        .prepare_media_edit("-42", "800", NativeMediaKind::File, "", None)
        .await
        .unwrap();
    let result = f.api.edit_media(&token(), edit).await.unwrap();
    let parsed = media_edit::snapshot(
        &result,
        "77",
        "-42",
        Some("7"),
        "800",
        NativeMediaKind::File,
    )
    .unwrap();
    assert_eq!(parsed.caption, "");
    assert_eq!(parsed.files[0].id, "stable");
    let calls = f.calls();
    assert_eq!(calls.len(), 1);
    assert!(calls[0].path.ends_with("/editMessageCaption"));
    let body: Value = serde_json::from_slice(&calls[0].body).unwrap();
    assert_eq!(
        body,
        json!({"chat_id":"-42","message_id":"800","caption":""})
    );
}
#[tokio::test]
async fn media_replacement_is_multipart_with_json_input_media_and_exact_bytes() {
    let f = Fixture::new().await;
    f.answer(Answer::json(
        StatusCode::OK,
        json!({"ok":true,"result":document("retained")}),
    ));
    let file = crate::media::outbound::PreparedFile {
        bytes: vec![1, 2, 3, 4],
        filename: "report.bin".into(),
        mime: "application/octet-stream".into(),
        caption: None,
    };
    let edit = f
        .api
        .prepare_media_edit("-42", "800", NativeMediaKind::File, "retained", Some(file))
        .await
        .unwrap();
    f.api.edit_media(&token(), edit).await.unwrap();
    let calls = f.calls();
    assert_eq!(calls.len(), 1);
    assert!(calls[0].path.ends_with("/editMessageMedia"));
    let body = String::from_utf8_lossy(&calls[0].body);
    assert!(body.contains("name=\"media\""));
    assert!(body.contains("\"media\":\"attach://replacement\""));
    assert!(body.contains("\"type\":\"document\""));
    assert!(body.contains("\"caption\":\"retained\""));
    assert!(body.contains("name=\"replacement\"; filename=\"report.bin\""));
    assert!(!body.contains("message_thread_id"));
    assert!(!body.contains("parse_mode"));
    assert!(calls[0].body.windows(4).any(|v| v == [1, 2, 3, 4]));
}
#[test]
fn media_receipts_refuse_boolean_wrong_topic_album_bot_and_unexpected_media() {
    for defect in [
        "boolean",
        "bot",
        "chat",
        "topic",
        "message",
        "album",
        "caption_entities",
        "family",
        "locator",
        "identity",
    ] {
        let mut body = document("caption");
        match defect {
            "boolean" => body = json!(true),
            "bot" => body["from"]["id"] = 78.into(),
            "chat" => body["chat"]["id"] = (-43).into(),
            "topic" => body["message_thread_id"] = 8.into(),
            "message" => body["message_id"] = 801.into(),
            "album" => body["media_group_id"] = "album".into(),
            "caption_entities" => {
                body["caption_entities"] = json!([{"type":"bold","offset":0,"length":1}])
            }
            "family" => body["photo"] = json!([]),
            "locator" => body["document"]["file_id"] = "../bad".into(),
            _ => body["document"]
                .as_object_mut()
                .unwrap()
                .remove("file_unique_id")
                .map(|_| ())
                .unwrap(),
        }
        assert_eq!(
            media_edit::snapshot(&body, "77", "-42", Some("7"), "800", NativeMediaKind::File),
            Err(Error::Unconfirmed),
            "{defect}"
        );
    }
}
#[test]
fn photos_are_one_attachment_with_bounded_unique_variants() {
    let mut body = document("");
    body.as_object_mut().unwrap().remove("document");
    body["photo"] = json!([{"file_id":"small","file_unique_id":"small_unique","width":90,"height":90},{"file_id":"large","file_unique_id":"large_unique","width":1280,"height":1280}]);
    let snapshot =
        media_edit::snapshot(&body, "77", "-42", Some("7"), "800", NativeMediaKind::Image).unwrap();
    assert_eq!(snapshot.files.len(), 2);
    body["photo"][1]["file_unique_id"] = "small_unique".into();
    assert!(
        media_edit::snapshot(&body, "77", "-42", Some("7"), "800", NativeMediaKind::Image).is_err()
    );
}
