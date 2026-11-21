//! Native Telegram media with `CMAEG_MEDIA_ENABLED`: PhotoSize choice, Bot API
//! locators and thumbnails, and normalized facts that never answer 401.
use crate::ingress::{message, Native};
use cmaeg_core::activity::{ActivityKind, Attachment, AttachmentKind, AttachmentSource};
use serde_json::{json, Value};

const SECRET: &str = "native-webhook-fixture";

/// The fixture message with `fields` in place of its text.
fn media(fields: Value) -> Value {
    let mut raw = message();
    let message = raw["message"].as_object_mut().unwrap();
    message.remove("text");
    message.extend(fields.as_object().unwrap().clone());
    raw
}
fn file(unique_id: &str, facts: Value) -> Value {
    let mut value =
        json!({"file_id": format!("BAACAgIA-{unique_id}"), "file_unique_id": unique_id});
    value
        .as_object_mut()
        .unwrap()
        .extend(facts.as_object().unwrap().clone());
    value
}
fn located(unique_id: &str) -> Option<AttachmentSource> {
    Some(AttachmentSource::Telegram {
        file_id: format!("BAACAgIA-{unique_id}"),
        file_unique_id: unique_id.into(),
    })
}
/// Text, attachments and album of the last launched message.
fn last(app: &Native) -> (String, Vec<Attachment>, Option<String>) {
    let requests = app.fixture.recorder.requests.lock().unwrap();
    match &requests.last().unwrap().activity.kind {
        ActivityKind::Message {
            text,
            attachments,
            album_id,
            ..
        } => (text.clone(), attachments.clone(), album_id.clone()),
        other => panic!("expected a message, got {other:?}"),
    }
}
fn count(rendered: &str, series: &str) -> u64 {
    rendered
        .lines()
        .find_map(|line| line.strip_prefix(series)?.trim().parse().ok())
        .unwrap_or(0)
}

#[tokio::test]
async fn media_updates_are_refused_as_before_while_media_is_off() {
    let app = Native::new(None).await;
    for raw in [
        media(
            json!({"photo": [file("AgADp90", json!({"width": 90, "height": 90}))], "caption": "hi"}),
        ),
        media(json!({"voice": file("AgADvoice", json!({"duration": 3}))})),
    ] {
        let response = app.send(&raw, SECRET).await;
        assert_eq!(response.status(), 202);
        assert_eq!(
            response.json::<Value>().await.unwrap(),
            json!({"status": "refused", "reason": "attachment_unsupported"})
        );
    }
    assert!(app.fixture.recorder.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn native_photos_use_the_largest_size_within_limits_else_the_smallest() {
    let app = Native::with_media().await;
    let size = |side: u64, bytes: u64| {
        let unique_id = format!("AgADp{side}");
        file(
            &unique_id,
            json!({"width": side, "height": side * 3 / 4, "file_size": bytes}),
        )
    };
    let sizes: Vec<_> = [90, 320, 800, 1280, 2560]
        .map(|side| size(side, side * 100))
        .into();
    app.accepted(&media(
        json!({"photo": sizes, "caption": "holiday", "media_group_id": "album-7"}),
    ))
    .await;
    let (text, attachments, album) = last(&app);
    assert_eq!(
        (text.as_str(), album.as_deref()),
        ("holiday", Some("album-7"))
    );
    assert_eq!(
        serde_json::to_value(&attachments).unwrap(),
        json!([{"reference": "telegram:AgADp1280", "kind": "image", "mime_type": "image/jpeg",
            "size_bytes": 128000, "variant": "photo", "width": 1280, "height": 960,
            "source": {"type": "telegram", "file_id": "BAACAgIA-AgADp1280", "file_unique_id": "AgADp1280"}}])
    );
    let oversized = [size(800, 80_000), size(1280, 9 * 1024 * 1024)];
    app.accepted(&media(json!({ "photo": oversized }))).await;
    assert_eq!(last(&app).1[0].width, Some(800));
    let large = [size(2560, 1_000), size(3000, 1_000), size(4096, 1_000)];
    app.accepted(&media(json!({ "photo": large }))).await;
    assert_eq!(last(&app).1[0].source, located("AgADp2560"));
}

#[tokio::test]
async fn native_media_keeps_thumbnails_document_kinds_and_degrades_bad_locators() {
    let metrics = cmaeg::telemetry::install().unwrap();
    let app = Native::with_media().await;
    let thumbnail = file("AQADthumb", json!({"width": 320, "height": 180}));
    let video = file(
        "AgADvideo",
        json!({"width": 640, "height": 360, "duration": 9, "thumbnail": thumbnail}),
    );
    app.accepted(&media(json!({ "video": video }))).await;
    let attachments = last(&app).1;
    assert_eq!(attachments[0].source, located("AgADvideo"));
    assert_eq!(attachments[0].thumbnail, located("AQADthumb"));
    let image = file(
        "AgADdoc",
        json!({"file_name": "chart.png", "mime_type": "image/png"}),
    );
    app.accepted(&media(json!({ "document": image }))).await;
    assert_eq!(last(&app).1[0].kind, AttachmentKind::Image);
    let series =
        "cmaeg_media_items_total{source=\"telegram\",kind=\"image\",outcome=\"invalid_locator\"}";
    let before = count(&metrics.render(), series);
    let invalid = json!({"file_id": "BAACAgIA-photo", "file_unique_id": "not valid!", "width": 90, "height": 90});
    app.accepted(&media(
        json!({"photo": [invalid], "caption": "still delivered"}),
    ))
    .await;
    let (text, attachments, _) = last(&app);
    assert_eq!(text, "still delivered");
    assert_eq!(attachments[0].reference, "telegram:item-0");
    assert_eq!(
        (&attachments[0].source, &attachments[0].thumbnail),
        (&None, &None)
    );
    assert_eq!(count(&metrics.render(), series), before + 1);
}

#[tokio::test]
async fn sender_declared_facts_never_make_a_native_update_answer_401() {
    let metrics = cmaeg::telemetry::install().unwrap();
    let repaired = |field: &str| {
        format!("cmaeg_media_facts_repaired_total{{source=\"telegram\",field=\"{field}\"}}")
    };
    let app = Native::with_media().await;
    let name = format!("{}.pdf", "文".repeat(100));
    let cases = [
        (
            Some("dimensions"),
            json!({"video": file("AgADvid", json!({"width": 0, "height": 0}))}),
        ),
        (
            Some("filename"),
            json!({"document": file("AgADdoc", json!({"file_name": name}))}),
        ),
        (
            Some("mime_type"),
            json!({"document": file("AgADdoc", json!({"mime_type": "text/plain; charset=utf-8"}))}),
        ),
        (
            Some("duration_seconds"),
            json!({"audio": file("AgADaud", json!({"duration": 100_000}))}),
        ),
        (
            Some("emoji"),
            json!({"sticker": file("AgADstk", json!({"width": 512, "height": 512, "emoji": "🙂".repeat(10)}))}),
        ),
        (
            None,
            json!({"document": {"file_id": "BAACAgIA-doc", "file_unique_id": "not valid!"}}),
        ),
    ];
    for (field, fields) in cases {
        let before = metrics.render();
        let response = app.send(&media(fields), SECRET).await;
        assert!(
            response.status().is_success(),
            "{field:?}: {}",
            response.status()
        );
        assert_eq!(
            response.json::<Value>().await.unwrap()["status"],
            "accepted",
            "{field:?}"
        );
        // An invalid identifier is a locator problem, not a fact repair.
        if let Some(field) = field {
            let after = metrics.render();
            assert_eq!(
                count(&after, &repaired(field)),
                count(&before, &repaired(field)) + 1
            );
        }
    }
    let attachments = last(&app).1;
    assert_eq!(
        (attachments[0].reference.as_str(), &attachments[0].source),
        ("telegram:item-0", &None)
    );
}
