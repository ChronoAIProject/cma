use super::*;
use crate::telegram::{
    media_fixture::{Answer, Fixture},
    Error,
};
use cmaeg_core::channel::Secret;
use serde_json::json;

type Case = (
    NativeMediaKind,
    &'static str,
    &'static str,
    &'static str,
    &'static [u8],
);
fn cases() -> Vec<Case> {
    vec![
        (
            NativeMediaKind::Voice,
            "sendVoice",
            "voice",
            "audio/ogg",
            include_bytes!("../../tests/fixtures/media/voice.ogg"),
        ),
        (
            NativeMediaKind::Animation,
            "sendAnimation",
            "animation",
            "image/gif",
            include_bytes!("../../tests/fixtures/media/anim.gif"),
        ),
        (
            NativeMediaKind::Animation,
            "sendAnimation",
            "animation",
            "video/mp4",
            include_bytes!("../../tests/fixtures/whatsapp-outbound/whatsapp-baseline.mp4"),
        ),
        (
            NativeMediaKind::Sticker,
            "sendSticker",
            "sticker",
            "image/webp",
            include_bytes!("../../tests/fixtures/whatsapp-outbound/whatsapp-static.webp"),
        ),
    ]
}
fn file(bytes: &[u8], mime: &str) -> PreparedFile {
    PreparedFile {
        bytes: bytes.to_vec(),
        filename: "attachment".into(),
        mime: mime.into(),
        caption: None,
    }
}

#[tokio::test]
async fn native_presentations_use_exact_methods_topics_bytes_and_confirmed_kind() {
    let f = Fixture::new().await;
    for (kind, method, field, mime, bytes) in cases() {
        let upload = f
            .api
            .prepare_media("-42", Some("7"), kind, file(bytes, mime))
            .await
            .unwrap();
        f.answer(Answer::json(
            reqwest::StatusCode::OK,
            json!({"ok":true,"result":{
                "message_id":800,"chat":{"id":-42},"message_thread_id":7,field:{"file_id":"file"}
            }}),
        ));
        assert_eq!(
            f.api
                .send_media(&Secret::new("77:fixture"), "77", upload)
                .await,
            Ok("800".into())
        );
        let call = f.calls().pop().unwrap();
        assert_eq!(call.path, format!("/bot77:fixture/{method}"));
        assert!(call.body.windows(bytes.len()).any(|part| part == bytes));
        let text = String::from_utf8_lossy(&call.body);
        assert!(text.contains(&format!("name=\"{field}\"; filename=")));
        assert!(text.contains(&format!("Content-Type: {mime}")));
        assert!(text.contains("name=\"message_thread_id\"\r\n\r\n7"));
        assert!(!text.contains("reply_markup") && !text.contains("77:fixture"));
    }
}

#[tokio::test]
async fn new_presentation_wrong_or_lost_receipt_remains_uncertain_without_retry() {
    let f = Fixture::new().await;
    for (_, _, _, mime, bytes) in cases().into_iter().take(1) {
        for defect in 0..3 {
            let mut value = json!({"ok":true,"result":{"message_id":800,"chat":{"id":-42},"message_thread_id":7,"voice":{"file_id":"file"}}});
            let mut answer = Answer::ok();
            match defect {
                0 => {
                    value["result"].as_object_mut().unwrap().remove("voice");
                }
                1 => value["result"]["message_thread_id"] = 8.into(),
                _ => answer.broken = true,
            }
            answer.body = serde_json::to_vec(&value).unwrap();
            f.answer(answer);
            let upload = f
                .api
                .prepare_media("-42", Some("7"), NativeMediaKind::Voice, file(bytes, mime))
                .await
                .unwrap();
            assert_eq!(
                f.api
                    .send_media(&Secret::new("77:fixture"), "77", upload)
                    .await,
                Err(Error::Unconfirmed)
            );
        }
    }
    assert_eq!(f.calls().len(), 3);
}

#[tokio::test]
async fn invalid_profiles_captions_and_controls_fail_before_transport() {
    let f = Fixture::new().await;
    for (kind, _, _, mime, bytes) in cases() {
        for defect in 0..4 {
            let mut candidate = file(bytes, mime);
            match defect {
                0 => {
                    candidate.bytes.pop();
                }
                1 => candidate.mime = "application/pdf".into(),
                2 => candidate.filename = "wrong.exe".into(),
                _ => candidate.bytes.clear(),
            }
            assert!(f
                .api
                .prepare_media("-42", Some("7"), kind, candidate)
                .await
                .is_err());
        }
        assert!(f
            .api
            .prepare_media_with_markup(
                "-42",
                Some("7"),
                kind,
                file(bytes, mime),
                Some(&json!({"inline_keyboard":[]}))
            )
            .await
            .is_err());
    }
    for (kind, bytes) in [
        (
            NativeMediaKind::Voice,
            include_bytes!("../../tests/fixtures/media/tone.mp3").as_slice(),
        ),
        (
            NativeMediaKind::Animation,
            include_bytes!("../../tests/fixtures/media/h264_aac.mp4").as_slice(),
        ),
        (
            NativeMediaKind::Sticker,
            include_bytes!("../../tests/fixtures/whatsapp-outbound/whatsapp-animated.webp")
                .as_slice(),
        ),
    ] {
        assert!(f
            .api
            .prepare_media(
                "-42",
                Some("7"),
                kind,
                file(bytes, "application/octet-stream")
            )
            .await
            .is_err());
    }
    let mut sticker = file(cases()[3].4, "image/webp");
    sticker.caption = Some("unsupported".into());
    assert!(f
        .api
        .prepare_media("-42", Some("7"), NativeMediaKind::Sticker, sticker)
        .await
        .is_err());
    assert!(f.calls().is_empty());
}

#[test]
fn gif_bounds_dimensions_frames_and_complete_container_without_pixel_allocation() {
    let valid = include_bytes!("../../tests/fixtures/media/anim.gif");
    assert!(gif(valid));
    for end in 0..valid.len() {
        assert!(!gif(&valid[..end]));
    }
    let mut invalid = valid.to_vec();
    invalid[6..8].copy_from_slice(&1921u16.to_le_bytes());
    assert!(!gif(&invalid));
    let mut trailing = valid.to_vec();
    trailing.push(0);
    assert!(!gif(&trailing));
}

#[test]
fn animation_frame_budget_is_checked_without_decoding_frames() {
    for count in [256, 257] {
        let mut bytes = Vec::new();
        image::codecs::gif::GifEncoder::new(&mut bytes)
            .encode_frames((0..count).map(|_| {
                image::Frame::new(image::RgbaImage::from_pixel(
                    1,
                    1,
                    image::Rgba([0, 0, 0, 255]),
                ))
            }))
            .unwrap();
        assert_eq!(gif(&bytes), count <= 256);
    }
}
