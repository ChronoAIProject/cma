//! Telegram media through the NyxID relay with `CMAEG_MEDIA_ENABLED=true`, sent
//! as NyxID serializes it. No attachment fact may turn a callback into a 400.
use super::{
    managed_creation::creation,
    nyxid_shapes::{
        attachment, counter, file, refused_media, telegram, telegram_document, telegram_photo,
        telegram_shapes, TELEGRAM_FILE_ID,
    },
    support::*,
};
use cmaeg::turns::{
    render_input::render,
    render_media::{Audience, RenderMedia},
};
use cmaeg_core::activity::{ActivityKind, Attachment, AttachmentKind, AttachmentSource};
use serde_json::{json, Value};

const MENTION: &str = "look at this @support_bot";

fn mention_policy(value: &mut Value) {
    value["sources"][0]["admission"] = json!({"type":"scoped","senders":{"type":"open"},"chats":{"type":"open"},"groups":"mention"});
}

fn raw(envelope: &mut Envelope) -> &mut Value {
    &mut envelope.payload["raw_platform_data"]["message"]
}

fn caption(envelope: &mut Envelope, caption: &str) {
    raw(envelope)["caption"] = caption.into();
    envelope.payload["content"]["text"] = caption.into();
}

fn shape(name: &str) -> Envelope {
    telegram_shapes()
        .into_iter()
        .find_map(|(shape, envelope)| (shape == name).then_some(envelope))
        .unwrap()
}

async fn accept(fixture: &Fixture, envelope: &Envelope, shape: &str) {
    let response = fixture.send(envelope).await;
    assert_eq!(response.status(), 202, "{shape}");
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["status"], "accepted", "{shape}: {body}");
}

async fn refusal(fixture: &Fixture, envelope: &Envelope, reason: &str) {
    let response = fixture.send(envelope).await;
    assert_eq!(response.status(), 202, "{reason}");
    assert_eq!(
        response.json::<Value>().await.unwrap(),
        json!({"status": "refused", "reason": reason})
    );
}

/// Text, attachments, album and mention flag of the last launched message.
fn last(fixture: &Fixture) -> (String, Vec<Attachment>, Option<String>, bool) {
    let requests = fixture.recorder.requests.lock().unwrap();
    match &requests.last().unwrap().activity.kind {
        ActivityKind::Message {
            text,
            attachments,
            album_id,
            mentions_bot,
            ..
        } => (
            text.clone(),
            attachments.clone(),
            album_id.clone(),
            *mentions_bot,
        ),
        other => panic!("expected a message, got {other:?}"),
    }
}

/// The provider input the last launched turn renders.
fn rendered(fixture: &Fixture) -> String {
    let requests = fixture.recorder.requests.lock().unwrap();
    let request = requests.last().unwrap();
    let audience = Audience {
        tools: false,
        downloads: false,
    };
    let input = render(
        &request.activity,
        &request.channel.definition,
        &RenderMedia::empty(None),
        audience,
    );
    input[0]["content"][0]["text"].as_str().unwrap().to_owned()
}

#[test]
fn nyxid_media_shapes_are_refused_off_and_admitted_on() {
    run_isolated("telegram_media::nyxid_media_shapes_are_refused_off_and_admitted_on_child");
}

#[tokio::test]
#[ignore = "run by the enclosing telemetry test in its own process"]
async fn nyxid_media_shapes_are_refused_off_and_admitted_on_child() {
    let off = Fixture::new(|_| {}, Default::default()).await;
    let on = Fixture::with_media(|_| {}, Default::default()).await;
    for (shape, envelope) in telegram_shapes() {
        refused_media(&off, &envelope, shape).await;
        accept(&on, &envelope, shape).await;
        let (_, attachments, _, _) = last(&on);
        assert_eq!(attachments.len(), 1, "{shape}");
        assert_eq!(
            attachments[0].source,
            Some(AttachmentSource::NyxidRelay { index: 0 }),
            "{shape}"
        );
    }
}

#[test]
fn captioned_relay_photo_is_correlated_on_refusal_and_delivered_when_enabled() {
    run_isolated("telegram_media::captioned_relay_photo_is_correlated_on_refusal_and_delivered_when_enabled_child");
}

#[tokio::test]
#[ignore = "run by the enclosing telemetry test in its own process"]
async fn captioned_relay_photo_is_correlated_on_refusal_and_delivered_when_enabled_child() {
    use axum::body::Body;
    use cmaeg::{http::callbacks, turns::observer::ActivityObserver};
    use cmaeg_core::ids::{EventId, RecordId};
    use std::sync::{Arc, Mutex};
    use tower::ServiceExt;

    #[derive(Default)]
    struct Refusals(Mutex<Vec<(Option<EventId>, String)>>);
    impl ActivityObserver for Refusals {
        fn refused(
            &self,
            _record: &RecordId,
            _source: &'static str,
            event: Option<&EventId>,
            reason: &str,
        ) {
            self.0.lock().unwrap().push((event.cloned(), reason.into()));
        }
    }

    let off = Fixture::new(|_| {}, Default::default()).await;
    let on = Fixture::with_media(|_| {}, Default::default()).await;
    let observer = Arc::new(Refusals::default());
    let mut photo = telegram_photo();
    caption(&mut photo, "Describe the text in this fixture image.");
    let (body, token) = photo.signed();
    let response = callbacks::router(off.state.clone().with_activity_observer(observer.clone()))
        .oneshot(off.local_request(Body::from(body), &token))
        .await
        .unwrap();

    assert_eq!(response.status(), 202);
    let body = axum::body::to_bytes(response.into_body(), 1024)
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap(),
        json!({"status": "refused", "reason": "attachment_unsupported"})
    );
    assert_eq!(
        *observer.0.lock().unwrap(),
        vec![(
            Some(EventId::new("message-1").unwrap()),
            "attachment_unsupported".into()
        )]
    );
    assert!(off.recorder.requests.lock().unwrap().is_empty());

    accept(&on, &photo, "captioned photo").await;
    let (text, attachments, _, _) = last(&on);
    assert_eq!(text, "Describe the text in this fixture image.");
    assert_eq!(attachments.len(), 1);
    assert_eq!(
        attachments[0].source,
        Some(AttachmentSource::NyxidRelay { index: 0 })
    );
    accept(&off, &Envelope::new(), "adjacent plain text").await;
    assert_eq!(last(&off).0, "hello");
}

#[tokio::test]
async fn a_captioned_group_photo_mentioning_the_bot_carries_its_photo() {
    let fixture = Fixture::with_media(mention_policy, Default::default()).await;
    let mut envelope = telegram_photo();
    caption(&mut envelope, MENTION);
    raw(&mut envelope)["caption_entities"] = json!([{"type":"mention","offset":13,"length":12}]);
    envelope.group();
    accept(&fixture, &envelope, "group photo").await;
    let (text, attachments, album, mentioned) = last(&fixture);
    assert_eq!(text, MENTION);
    assert!(mentioned);
    assert_eq!(album, None);
    assert_eq!(
        serde_json::to_value(&attachments).unwrap(),
        json!([{"reference": "telegram:AgADfixture", "kind": "image", "mime_type": "image/jpeg",
            "size_bytes": 81234, "source": {"type": "nyxid_relay", "index": 0},
            "variant": "photo", "width": 1280, "height": 960}])
    );
    let input = rendered(&fixture);
    assert!(input.starts_with("look at this"), "{input}");
    assert!(input.contains("[Attachments: 1"), "{input}");
}

#[tokio::test]
async fn a_group_photo_without_a_caption_mention_is_not_addressed() {
    let fixture = Fixture::with_media(mention_policy, Default::default()).await;
    let mut envelope = telegram_photo();
    caption(&mut envelope, MENTION);
    // Text entities do not describe a caption, so they are not caption evidence.
    raw(&mut envelope)["entities"] = json!([{"type":"mention","offset":13,"length":12}]);
    envelope.group();
    refusal(&fixture, &envelope, "group_not_addressed").await;
    assert!(fixture.recorder.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_private_voice_note_without_a_caption_renders_a_non_blank_input() {
    let fixture = Fixture::with_media(|_| {}, Default::default()).await;
    accept(&fixture, &shape("voice"), "voice").await;
    let (text, attachments, _, _) = last(&fixture);
    assert_eq!(text, "");
    assert_eq!(attachments[0].kind, AttachmentKind::Audio);
    assert_eq!(attachments[0].duration_seconds, Some(3));
    let input = rendered(&fixture);
    assert!(input.starts_with("[Attachments: 1"), "{input}");
    assert!(input.contains("voice message"), "{input}");
}

#[tokio::test]
async fn animations_albums_and_animated_stickers_keep_their_platform_facts() {
    let fixture = Fixture::with_media(|_| {}, Default::default()).await;
    accept(
        &fixture,
        &shape("animation sharing its document's file_id"),
        "animation",
    )
    .await;
    let attachments = last(&fixture).1;
    assert_eq!(attachments.len(), 1);
    assert_eq!(
        serde_json::to_value(&attachments[0]).unwrap()["variant"],
        "animation"
    );
    let mut album = telegram_photo();
    raw(&mut album)["media_group_id"] = "13579246".into();
    accept(&fixture, &album, "album item").await;
    assert_eq!(last(&fixture).2.as_deref(), Some("13579246"));
    let sticker = telegram(
        json!({"sticker": file(json!({"type": "regular", "width": 512, "height": 512, "is_animated": true, "is_video": false}))}),
        "image",
        None,
        vec![attachment(0, "image", TELEGRAM_FILE_ID, json!({}))],
    );
    accept(&fixture, &sticker, "animated sticker").await;
    assert_eq!(last(&fixture).1[0].kind, AttachmentKind::Other);
}

#[test]
fn an_envelope_item_matching_no_file_leaves_the_attachment_unlocated() {
    run_isolated(
        "telegram_media::an_envelope_item_matching_no_file_leaves_the_attachment_unlocated_child",
    );
}

#[tokio::test]
#[ignore = "run by the enclosing telemetry test in its own process"]
async fn an_envelope_item_matching_no_file_leaves_the_attachment_unlocated_child() {
    let metrics = cmaeg::telemetry::install().unwrap();
    let series =
        |outcome: &str| format!("cmaeg_media_relay_correlation_total{{outcome=\"{outcome}\"}}");
    let fixture = Fixture::with_media(|_| {}, Default::default()).await;
    let mut envelope = telegram_document();
    envelope.payload["content"]["attachments"][0]["url"] = "AgACAgIAAxkBAAIBX2Zx-another".into();
    let before = metrics.render();
    accept(&fixture, &envelope, "unmatched").await;
    let after = metrics.render();
    let attachments = last(&fixture).1;
    assert_eq!(attachments[0].source, None);
    assert_eq!(attachments[0].reference, "telegram:AgADfixture");
    assert_eq!(
        counter(&after, &series("unmatched")),
        counter(&before, &series("unmatched")) + 1
    );
    assert_eq!(
        counter(&after, &series("matched")),
        counter(&before, &series("matched"))
    );
    assert!(rendered(&fixture).contains("not retrievable through this channel"));
}

#[tokio::test]
async fn an_envelope_that_disagrees_with_the_raw_message_is_content_unsupported() {
    let fixture = Fixture::with_media(|_| {}, Default::default()).await;
    let mut kind = telegram_photo();
    kind.payload["content"]["type"] = "file".into();
    let mut text = telegram_photo();
    text.payload["content"]["text"] = "something else".into();
    let mut missing = telegram_photo();
    missing.payload["content"]["text"] = Value::Null;
    let mut long = telegram_photo();
    caption(&mut long, &"a".repeat(32 * 1024 + 1));
    for envelope in [&kind, &text, &missing, &long] {
        refusal(&fixture, envelope, "content_unsupported").await;
    }
    assert!(fixture.recorder.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn attachments_without_raw_media_or_without_any_usable_item_are_refused() {
    let fixture = Fixture::with_media(|_| {}, Default::default()).await;
    let mut listed = Envelope::new();
    listed.payload["content"]["attachments"] =
        json!([attachment(0, "image", TELEGRAM_FILE_ID, json!({}))]);
    let mut malformed = Envelope::new();
    malformed.payload["content"]["attachments"] = json!(7);
    let mut unusable = telegram_photo();
    raw(&mut unusable)["photo"] = "not sizes".into();
    for envelope in [&listed, &malformed, &unusable] {
        refusal(&fixture, envelope, "attachment_unsupported").await;
    }
    assert!(fixture.recorder.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn edits_bot_senders_and_managed_bot_creation_behave_as_before() {
    let fixture = Fixture::with_media(|_| {}, Default::default()).await;
    let mut edit = telegram_photo();
    let message = raw(&mut edit).take();
    edit.payload["raw_platform_data"] = json!({ "edited_message": message });
    refusal(&fixture, &edit, "message_edit").await;
    let mut bot = telegram_photo();
    raw(&mut bot)["from"]["is_bot"] = true.into();
    refusal(&fixture, &bot, "bot_sender").await;
    assert!(fixture.recorder.requests.lock().unwrap().is_empty());
    accept(&fixture, &creation(), "managed bot creation").await;
    let requests = fixture.recorder.requests.lock().unwrap();
    assert!(matches!(&requests[0].activity.kind,
        ActivityKind::Event { ce, attachments } if ce.r#type == "telegram.managed_bot_created" && attachments.is_empty()));
}

/// A raw Telegram fact outside the attachment domain, the repaired field it must
/// report (or none) and a check of the kept attachment.
type Case = (
    &'static str,
    Envelope,
    Option<&'static str>,
    fn(&Attachment),
);

fn normalization_cases() -> Vec<Case> {
    let item = |content_type| vec![attachment(0, content_type, TELEGRAM_FILE_ID, json!({}))];
    let cjk_name = format!("{}.pdf", "文".repeat(100));
    vec![
        (
            "zero video dimensions",
            telegram(
                json!({"video": file(json!({"width": 0, "height": 0, "duration": 9}))}),
                "video",
                None,
                item("video"),
            ),
            Some("dimensions"),
            |a| {
                assert_eq!(
                    (a.width, a.height, a.duration_seconds),
                    (None, None, Some(9))
                )
            },
        ),
        (
            "300-byte CJK document name",
            telegram(
                json!({"document": file(json!({"file_name": cjk_name}))}),
                "file",
                None,
                item("file"),
            ),
            Some("filename"),
            |a| {
                let name = a.filename.as_deref().unwrap();
                assert!(
                    name.len() <= 255 && name.ends_with("….pdf") && name.starts_with('文'),
                    "{name}"
                );
            },
        ),
        (
            "MIME type with parameters",
            telegram(
                json!({"document": file(json!({"mime_type": "text/plain; charset=utf-8"}))}),
                "file",
                None,
                item("file"),
            ),
            Some("mime_type"),
            |a| assert_eq!(a.mime_type.as_deref(), Some("text/plain")),
        ),
        (
            "audio lasting 100000 seconds",
            telegram(
                json!({"audio": file(json!({"duration": 100_000}))}),
                "audio",
                None,
                item("audio"),
            ),
            Some("duration_seconds"),
            |a| assert_eq!(a.duration_seconds, None),
        ),
        (
            "invalid file_unique_id",
            telegram(
                json!({"document": {"file_id": TELEGRAM_FILE_ID, "file_unique_id": "not valid!"}}),
                "file",
                None,
                item("file"),
            ),
            None,
            |a| {
                assert_eq!(a.reference, "telegram:item-0");
                assert_eq!(a.source, Some(AttachmentSource::NyxidRelay { index: 0 }));
            },
        ),
        (
            "40-byte sticker emoji",
            telegram(
                json!({"sticker": file(json!({"width": 512, "height": 512, "emoji": "🙂".repeat(10)}))}),
                "image",
                None,
                item("image"),
            ),
            Some("emoji"),
            |a| assert_eq!((a.emoji.as_deref(), a.width), (None, Some(512))),
        ),
    ]
}

#[test]
fn sender_declared_facts_are_normalized_and_never_reject_the_callback() {
    run_isolated(
        "telegram_media::sender_declared_facts_are_normalized_and_never_reject_the_callback_child",
    );
}

#[tokio::test]
#[ignore = "run by the enclosing telemetry test in its own process"]
async fn sender_declared_facts_are_normalized_and_never_reject_the_callback_child() {
    let metrics = cmaeg::telemetry::install().unwrap();
    let repaired = |field: &str| {
        format!("cmaeg_media_facts_repaired_total{{source=\"nyxid_relay\",field=\"{field}\"}}")
    };
    let fixture = Fixture::with_media(|_| {}, Default::default()).await;
    for (case, envelope, field, check) in normalization_cases() {
        let before = metrics.render();
        accept(&fixture, &envelope, case).await;
        let after = metrics.render();
        check(&last(&fixture).1[0]);
        for label in [
            "dimensions",
            "filename",
            "mime_type",
            "duration_seconds",
            "emoji",
        ] {
            let increase = u64::from(field == Some(label));
            assert_eq!(
                counter(&after, &repaired(label)),
                counter(&before, &repaired(label)) + increase,
                "{case}: {label}"
            );
        }
    }
}
