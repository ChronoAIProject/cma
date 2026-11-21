//! Automatic entities are exact UTF-16 spans; replacing text clears prior spans.
use super::*;
use crate::native_media::model::Presentation;

fn observed(body: &Value) -> Result<crate::native_media::model::Observation, Error> {
    media_edit::observe(body, "77", "-42", Some("7"), "800", NativeMediaKind::File)
}
#[test]
fn automatic_entities_validate_utf16_and_reject_ambiguous_spans() {
    let mut body = document("😀 https://example.com #tag");
    body["caption_entities"] = json!([
        {"type":"hashtag","offset":23,"length":4},
        {"type":"url","offset":3,"length":19}
    ]);
    let snapshot = observed(&body).unwrap().snapshot;
    let Some(Presentation::Telegram { entities, .. }) = snapshot.presentation else {
        panic!()
    };
    assert_eq!(entities[0].offset, 3);
    for entity in [
        json!({"type":"url","offset":1,"length":1}),
        json!({"type":"url","offset":3,"length":0}),
        json!({"type":"url","offset":3,"length":25}),
        json!({"type":"url","offset":u32::MAX,"length":2}),
        json!({"type":"text_link","offset":3,"length":19}),
        json!({"type":"url","offset":3,"length":19,"url":"https://other.example"}),
    ] {
        body["caption_entities"] = json!([entity]);
        assert!(observed(&body).is_err());
    }
    for entities in [
        json!([{"type":"url","offset":3,"length":19},{"type":"url","offset":3,"length":19}]),
        json!([{"type":"url","offset":3,"length":19},{"type":"hashtag","offset":21,"length":4}]),
    ] {
        body["caption_entities"] = entities;
        assert!(observed(&body).is_err());
    }
}
#[test]
fn automatic_entity_retention_limit_does_not_make_a_proved_caption_ambiguous() {
    let mut body = document(&"a ".repeat(129));
    body["caption_entities"] = Value::Array(
        (0..129)
            .map(|i| json!({"type":"mention","offset":2*i,"length":1}))
            .collect(),
    );
    assert!(!observed(&body).unwrap().eligible);
    assert!(
        media_edit::snapshot(&body, "77", "-42", Some("7"), "800", NativeMediaKind::File).is_err()
    );
}
#[tokio::test]
async fn decorated_photo_replacement_preserves_entities_placement_and_spoiler() {
    let mut body = document("😀 https://example.com");
    body.as_object_mut().unwrap().remove("document");
    body["photo"] =
        json!([{"file_id":"photo","file_unique_id":"photo_unique","width":20,"height":20}]);
    body["caption_entities"] = json!([{"type":"url","offset":3,"length":19}]);
    body["show_caption_above_media"] = true.into();
    body["has_media_spoiler"] = true.into();
    let current =
        media_edit::snapshot(&body, "77", "-42", Some("7"), "800", NativeMediaKind::Image).unwrap();
    for replace_caption in [false, true] {
        let fixture = Fixture::new().await;
        fixture.answer(Answer::json(
            StatusCode::OK,
            json!({"ok":true,"result":body}),
        ));
        let mut patch =
            json!({"replacement":{"kind":"image","source":{"type":"base64","data":"AQIDBA=="}}});
        if replace_caption {
            patch["caption"] = "new caption".into();
        }
        let patch = serde_json::from_value(patch).unwrap();
        let file = crate::media::outbound::PreparedFile {
            bytes: vec![1, 2, 3, 4],
            filename: "photo.png".into(),
            mime: "image/png".into(),
            caption: None,
        };
        let edit = fixture
            .api
            .prepare_media_edit("-42", "800", &current, &patch, Some(file))
            .await
            .unwrap();
        fixture.api.edit_media(&token(), edit).await.unwrap();
        let calls = fixture.calls();
        let wire = String::from_utf8_lossy(&calls[0].body);
        let media: Value = serde_json::from_str(
            wire.split("name=\"media\"\r\n\r\n")
                .nth(1)
                .unwrap()
                .split("\r\n--")
                .next()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            media["caption_entities"],
            if replace_caption {
                json!([])
            } else {
                body["caption_entities"].clone()
            }
        );
        assert_eq!(media["show_caption_above_media"], true);
        assert_eq!(media["has_spoiler"], true);
        assert!(media.get("parse_mode").is_none());
    }
}
#[test]
fn retained_asset_metadata_ignores_only_nested_file_locators() {
    let mut body = document("caption");
    body["document"]["thumbnail"] =
        json!({"file_id":"old","file_unique_id":"thumb","width":40,"height":30});
    let old = observed(&body).unwrap().snapshot;
    body["document"]["thumbnail"]["file_id"] = "new".into();
    assert!(old.same_files(&observed(&body).unwrap().snapshot));
    body["document"]["thumbnail"]["file_unique_id"] = "other".into();
    assert!(!old.same_files(&observed(&body).unwrap().snapshot));
    body["has_media_spoiler"] = true.into();
    assert!(observed(&body).is_err());
}
