use super::{
    discord_controls_tests::sender,
    discord_media_controls_tests::{media, send},
    intents::DeliveryOutcome,
    send::Sender,
};
use crate::discord::components_fixture::FixtureState;
use axum::http::StatusCode;
use cmaeg_core::{message::OutboundMessage, seal::MessageRefPayload};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

pub(super) struct Native {
    pub message: Arc<Mutex<Value>>,
    pub fault: Arc<Mutex<Option<&'static str>>>,
}
pub(super) fn native(f: &FixtureState) -> Native {
    let current = Arc::new(Mutex::new(Value::Null));
    let fault = Arc::new(Mutex::new(None));
    let state = Native {
        message: current.clone(),
        fault: fault.clone(),
    };
    let memory = Arc::downgrade(&f.rest.memory);
    let records = f.setup.fixture.memory.clone();
    let metadata = if f.thread_kind.is_some() {
        f.thread_metadata()
    } else {
        json!({"id":"99","type":1})
    };
    *f.rest.memory.on_call.lock().unwrap() = Some(Arc::new(move |call| {
        let mut status = StatusCode::OK;
        let body = if call.method == "POST" && call.path.ends_with("/messages") {
            let mut body = multipart(&call.body);
            body["id"] = "800".into();
            body["channel_id"] = "99".into();
            body["author"] = json!({"id":"77","bot":true});
            body["type"] = 19.into();
            body["attachments"] = json!([{"id":"900","filename":"report.bin",
                "content_type":"application/octet-stream","size":4,
                "description":"Review this upload"}]);
            *current.lock().unwrap() = body.clone();
            body
        } else if call.method == "PATCH" {
            let upload = call
                .content_type
                .as_ref()
                .is_some_and(|v| v.starts_with("multipart/"));
            let patch = if upload {
                multipart(&call.body)
            } else {
                serde_json::from_slice(&call.body).unwrap()
            };
            assert!(patch.get("embeds").is_none());
            assert!(patch.get("message_reference").is_none());
            assert_eq!(
                patch["allowed_mentions"],
                json!({"parse":[],"replied_user":false})
            );
            let mut body = current.lock().unwrap();
            body["content"] = patch["content"].clone();
            body["components"] = patch["components"].clone();
            if upload {
                assert_eq!(patch["attachments"].as_array().unwrap().len(), 1);
                assert_eq!(patch["attachments"][0]["id"], 0);
                let wire = String::from_utf8_lossy(&call.body);
                assert_eq!(wire.matches("name=\"files[0]\"").count(), 1);
                assert!(!wire.contains("name=\"files[1]\""));
                let part = wire.split("name=\"files[0]\"").nth(1).unwrap();
                let bytes = part
                    .split("\r\n\r\n")
                    .nth(1)
                    .unwrap()
                    .split("\r\n--")
                    .next()
                    .unwrap();
                let mime = part
                    .split("Content-Type: ")
                    .nth(1)
                    .or_else(|| part.split("content-type: ").nth(1))
                    .unwrap()
                    .split("\r\n")
                    .next()
                    .unwrap();
                let next = body["attachments"][0]["id"]
                    .as_str()
                    .unwrap()
                    .parse::<u64>()
                    .unwrap()
                    + 1;
                body["attachments"] = patch["attachments"].clone();
                body["attachments"][0]["id"] = next.to_string().into();
                body["attachments"][0]["size"] = bytes.len().into();
                body["attachments"][0]["content_type"] = mime.into();
                if patch["attachments"][0]["is_spoiler"] == true {
                    body["attachments"][0]["flags"] = 8.into();
                }
            } else {
                assert!(patch.get("attachments").is_none());
            }
            let mut response = body.clone();
            if let Some(field) = *fault.lock().unwrap() {
                match field {
                    "500" => status = StatusCode::INTERNAL_SERVER_ERROR,
                    "generated" => {
                        response["attachments"][0]["width"] = 640.into();
                        response["attachments"][0]["height"] = 480.into();
                        response["embeds"] = json!([{"type":"image","thumbnail":{
                            "url":"https://cdn.discordapp.com/attachments/99/901/new.png",
                            "width":640,"height":480
                        }}]);
                        *body = response.clone();
                    }
                    "bind_before" | "bind_after" => {
                        let custom = body["components"][0]["components"][0]["custom_id"]
                            .as_str()
                            .unwrap();
                        let id = uuid::Uuid::parse_str(custom.split(':').nth(1).unwrap())
                            .unwrap()
                            .to_string();
                        *records.fault.lock().unwrap() = Some((id, field == "bind_after"));
                    }
                    _ => corrupt(&mut response, field),
                }
            }
            response
        } else if call.method == "POST" {
            status = StatusCode::NO_CONTENT;
            Value::Null
        } else if call.path.ends_with("/messages/800") {
            current.lock().unwrap().clone()
        } else {
            metadata.clone()
        };
        memory
            .upgrade()
            .unwrap()
            .responses
            .lock()
            .unwrap()
            .push_back((status, body));
    }));
    state
}
fn multipart(bytes: &[u8]) -> Value {
    let wire = String::from_utf8_lossy(bytes);
    serde_json::from_str(
        wire.split("name=\"payload_json\"\r\n\r\n")
            .nth(1)
            .unwrap()
            .split("\r\n--")
            .next()
            .unwrap(),
    )
    .unwrap()
}
pub(super) fn corrupt(body: &mut Value, field: &str) {
    match field {
        "old_id" => body["attachments"][0]["id"] = "900".into(),
        "size" => body["attachments"][0]["size"] = 99.into(),
        "filename" => body["attachments"][0]["filename"] = "wrong.bin".into(),
        "mime" => body["attachments"][0]["content_type"] = "text/plain".into(),
        "caption" => body["attachments"][0]["description"] = "wrong".into(),
        "extra" => {
            body["attachments"]
                .as_array_mut()
                .unwrap()
                .push(json!({"id":"999"}));
        }
        "missing" => body["attachments"] = json!([]),
        "controls" => body["components"] = json!([]),
        "content" => body["content"] = "wrong".into(),
        "reply" => body["message_reference"]["message_id"] = "999".into(),
        "type" => body["type"] = 0.into(),
        "flags" => body["flags"] = 4.into(),
        "spoiler" => body["attachments"][0]["flags"] = 8.into(),
        "channel" => body["channel_id"] = "100".into(),
        "author" => body["author"]["id"] = "78".into(),
        _ => panic!("unknown fault"),
    }
}
pub(super) fn editing(f: &FixtureState) -> Sender {
    let mut sender = sender(f, true);
    sender.relay = sender
        .relay
        .with_discord_media_controls(true)
        .with_discord_media_card_edits(true)
        .with_discord_card_edits(true)
        .with_discord_media_card_replacements(true)
        .with_consumer_interactions(true);
    sender
}
pub(super) async fn issued(f: &FixtureState, sender: &Sender) -> MessageRefPayload {
    let result = send(f, sender, &media(false, "file", &[1, 2, 3, 4]), true).await;
    assert!(
        matches!(result.delivery, DeliveryOutcome::Confirmed { .. }),
        "{:?}",
        result.delivery
    );
    result.message_ref.unwrap().open(&f.keys()).unwrap().payload
}
pub(super) fn replacement(kind: &str) -> OutboundMessage {
    let mut message = media(false, kind, b"replacement bytes");
    message.native_attachments[0].filename = Some("replacement.bin".into());
    message.native_attachments[0].caption = Some("Updated upload".into());
    message
}
