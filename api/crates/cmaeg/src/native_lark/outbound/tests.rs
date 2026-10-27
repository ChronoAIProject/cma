use super::*;
use crate::{
    native_lark::test_support::credentials,
    net::exchange::{BodyError, HttpReply, TransportError},
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use cmaeg_core::{channel::LarkRegion, message::NativeMediaSource};
use reqwest::StatusCode;
use serde_json::{json, Value};
#[path = "fixture.rs"]
pub(crate) mod fixture;
use fixture::*;

pub(crate) async fn prepared(item: &NativeAttachment) -> Result<MediaUpload, Error> {
    let file = crate::media::outbound::prepare(std::slice::from_ref(item))
        .await
        .unwrap()
        .pop()
        .unwrap();
    Client::prepare_media(item, file).await
}
#[tokio::test]
async fn native_lark_upload_contracts_cover_each_profile_region_and_thread() {
    for region in [LarkRegion::Lark, LarkRegion::Feishu] {
        for thread in [false, true] {
            for mime in MIME_TYPES {
                let item = attachment(mime);
                let mut destination = reply();
                destination.identity.region = region;
                destination.thread_id = thread.then(|| "omt_thread".into());
                let image = item.kind == NativeMediaKind::Image;
                let io = Api::new(vec![uploaded(image), confirmed("om_media", thread)]);
                let client = Client::production().unwrap().with_transport(io.clone());
                let token = client
                    .verified_token(&destination.identity, &credentials())
                    .await
                    .unwrap();
                let media = client
                    .upload_media(&token, &destination, prepared(&item).await.unwrap())
                    .await
                    .unwrap();
                assert_eq!(
                    client.send_media(&token, &destination, media).await,
                    Ok("om_media".into())
                );
                let calls = io.writes.lock().unwrap();
                assert_eq!(calls.len(), 2);
                for (url, _, _) in calls.iter() {
                    assert_eq!(url.origin().ascii_serialization(), region.origin());
                }
                assert_eq!(
                    calls[0].0.path(),
                    if image {
                        "/open-apis/im/v1/images"
                    } else {
                        "/open-apis/im/v1/files"
                    }
                );
                assert!(calls[0].1["content-type"]
                    .to_str()
                    .unwrap()
                    .starts_with("multipart/form-data; boundary="));
                let multipart = String::from_utf8_lossy(&calls[0].2);
                let file_type = match mime {
                    "application/pdf" => "pdf",
                    "text/plain" => "stream",
                    "audio/ogg" => "opus",
                    "video/mp4" => "mp4",
                    _ => "message",
                };
                let field = if image { "image_type" } else { "file_type" };
                assert!(multipart.contains(&format!("name=\"{field}\"\r\n\r\n{file_type}")));
                assert!(multipart.contains(&format!("Content-Type: {mime}")));
                assert!(!multipart.contains(token.as_str()));
                let NativeMediaSource::Base64 { data } = &item.source else {
                    panic!()
                };
                let bytes = STANDARD.decode(data).unwrap();
                assert!(calls[0].2.windows(bytes.len()).any(|v| v == bytes));
                let body: Value = serde_json::from_slice(&calls[1].2).unwrap();
                assert_eq!(
                    body["msg_type"],
                    match item.kind {
                        NativeMediaKind::Image => "image",
                        NativeMediaKind::File => "file",
                        NativeMediaKind::Audio => "audio",
                        _ => "media",
                    }
                );
                let content: Value =
                    serde_json::from_str(body["content"].as_str().unwrap()).unwrap();
                assert_eq!(
                    content,
                    if image {
                        json!({"image_key":"img_uploaded"})
                    } else {
                        json!({"file_key":"file_uploaded"})
                    }
                );
                if thread {
                    assert_eq!(
                        calls[1].0.path(),
                        "/open-apis/im/v1/messages/om_original/reply"
                    );
                    assert_eq!(body["reply_in_thread"], true);
                    assert!(body.get("receive_id").is_none());
                } else {
                    assert_eq!(calls[1].0.query(), Some("receive_id_type=chat_id"));
                    assert_eq!(body["receive_id"], "oc_native_chat");
                }
            }
        }
    }
}
#[tokio::test]
async fn native_lark_rejects_unsupported_or_mislabeled_batches_before_upload() {
    for mime in MIME_TYPES {
        for defect in ["mime", "extension", "caption", "kind", "payload"] {
            let mut item = attachment(mime);
            match defect {
                "mime" => item.mime_type = Some("application/octet-stream".into()),
                "extension" => item.filename = Some("misleading.bin".into()),
                "caption" => item.caption = Some("unsupported native caption".into()),
                "kind" => item.kind = NativeMediaKind::Sticker,
                _ => {
                    item.source = NativeMediaSource::Base64 {
                        data: STANDARD.encode(b"\0invalid"),
                    }
                }
            }
            assert!(
                matches!(prepared(&item).await, Err(Error::Invalid)),
                "{mime} {defect}"
            );
        }
    }
    for (kind, bytes) in [
        (
            NativeMediaKind::Audio,
            &include_bytes!("../../../tests/fixtures/media/tone.mp3")[..],
        ),
        (
            NativeMediaKind::Video,
            &include_bytes!("../../../tests/fixtures/whatsapp-outbound/whatsapp-baseline-aac.3gp")
                [..],
        ),
    ] {
        let mut item = attachment("audio/ogg");
        item.kind = kind;
        item.mime_type = None;
        item.source = NativeMediaSource::Base64 {
            data: STANDARD.encode(bytes),
        };
        assert!(matches!(prepared(&item).await, Err(Error::Invalid)));
    }
    let item = attachment("text/plain");
    let oversized = PreparedFile {
        bytes: vec![b'x'; MAX_OUTBOUND_BYTES as usize + 1],
        filename: "x.txt".into(),
        mime: "text/plain".into(),
        caption: None,
    };
    assert!(matches!(
        Client::prepare_media(&item, oversized).await,
        Err(Error::Invalid)
    ));
}
#[tokio::test]
async fn native_lark_upload_keys_are_exact_destination_bound_and_not_reusable_elsewhere() {
    for field in [
        "chat", "thread", "message", "app", "bot", "tenant", "region", "revision",
    ] {
        let destination = reply();
        let io = Api::new(vec![uploaded(false)]);
        let client = Client::production().unwrap().with_transport(io.clone());
        let token = Secret::new("fixture-native-access-token");
        let media = client
            .upload_media(
                &token,
                &destination,
                prepared(&attachment("application/pdf")).await.unwrap(),
            )
            .await
            .unwrap();
        let mut other = destination;
        match field {
            "chat" => other.chat_id = "oc_elsewhere".into(),
            "thread" => other.thread_id = Some("omt_other".into()),
            "message" => other.message_id = "om_other".into(),
            "app" => other.identity.app_id = "cli_other".into(),
            "bot" => other.identity.bot_open_id = "ou_other".into(),
            "tenant" => other.identity.tenant_key = "other_tenant".into(),
            "region" => other.identity.region = LarkRegion::Lark,
            _ => other.revision = "1".repeat(64),
        }
        assert_eq!(
            client.send_media(&token, &other, media).await,
            Err(Error::Invalid)
        );
        assert_eq!(io.writes.lock().unwrap().len(), 1);
    }
}
#[tokio::test]
async fn native_lark_uncertain_uploads_are_single_attempt_and_produce_no_sendable_key() {
    let responses = vec![
        answer(json!({"code":0,"data":{"file_key":"https://foreign.invalid/key"}})),
        answer(json!({"code":0,"data":{"image_key":"img_wrong_kind"}})),
        answer(json!({"code":123})),
        Ok(HttpReply::new(StatusCode::TEMPORARY_REDIRECT, vec![])),
        Ok(HttpReply::new(StatusCode::TOO_MANY_REQUESTS, vec![])),
        Ok(HttpReply {
            status: StatusCode::OK,
            headers: HeaderMap::new(),
            body: Err(BodyError::Oversized),
        }),
        Ok(HttpReply::new(StatusCode::OK, vec![b'x'; 64 * 1024 + 1])),
        Err(TransportError::Unavailable),
    ];
    for response in responses {
        let io = Api::new(vec![response]);
        let client = Client::production().unwrap().with_transport(io.clone());
        assert!(client
            .upload_media(
                &Secret::new("fixture-native-access-token"),
                &reply(),
                prepared(&attachment("application/pdf")).await.unwrap()
            )
            .await
            .is_err());
        assert_eq!(io.writes.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn native_lark_media_receipts_require_exact_new_message_chat_and_thread_shape() {
    for thread in [false, true] {
        for defect in [
            "chat",
            "original",
            "missing_id",
            "foreign_thread",
            "thread_shape",
            "thread_missing",
        ] {
            let mut destination = reply();
            destination.thread_id = thread.then(|| "omt_thread".into());
            let mut response =
                json!({"code":0,"data":{"message_id":"om_sent","chat_id":"oc_native_chat"}});
            if thread {
                response["data"]["thread_id"] = "omt_thread".into();
            }
            match defect {
                "chat" => response["data"]["chat_id"] = "oc_foreign".into(),
                "original" => response["data"]["message_id"] = "om_original".into(),
                "missing_id" => response["data"]
                    .as_object_mut()
                    .unwrap()
                    .remove("message_id")
                    .map(|_| ())
                    .unwrap(),
                "foreign_thread" => response["data"]["thread_id"] = "omt_foreign".into(),
                "thread_shape" => response["data"]["thread_id"] = 42.into(),
                _ if thread => {
                    response["data"]
                        .as_object_mut()
                        .unwrap()
                        .remove("thread_id");
                }
                _ => continue,
            }
            let io = Api::new(vec![uploaded(false), answer(response)]);
            let client = Client::production().unwrap().with_transport(io.clone());
            let token = Secret::new("fixture-native-access-token");
            let media = client
                .upload_media(
                    &token,
                    &destination,
                    prepared(&attachment("application/pdf")).await.unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                client.send_media(&token, &destination, media).await,
                Err(Error::Unconfirmed),
                "{thread} {defect}"
            );
            assert_eq!(io.writes.lock().unwrap().len(), 2);
        }
    }
}

#[tokio::test]
async fn native_lark_upload_retains_documented_bare_and_bounded_resource_keys() {
    for key in [
        "75235e0c-4f92-430a-a99b-8446610223cg".to_owned(),
        "k".repeat(1024),
    ] {
        let io = Api::new(vec![
            answer(json!({"code":0,"data":{"file_key":key}})),
            confirmed("om_sent", false),
        ]);
        let client = Client::production().unwrap().with_transport(io.clone());
        let token = Secret::new("fixture-native-access-token");
        let media = client
            .upload_media(
                &token,
                &reply(),
                prepared(&attachment("application/pdf")).await.unwrap(),
            )
            .await
            .unwrap();
        client.send_media(&token, &reply(), media).await.unwrap();
        let calls = io.writes.lock().unwrap();
        let body: Value = serde_json::from_slice(&calls[1].2).unwrap();
        assert_eq!(body["content"], json!({"file_key":key}).to_string());
    }
}
