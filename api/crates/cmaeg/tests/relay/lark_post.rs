//! Lark rich-text posts through the NyxID relay, sent as NyxID serializes them:
//! `content.type` is `unknown` with null text, so with `CMAEG_MEDIA_ENABLED=true`
//! the observer flattens the post's text from the raw event.
use super::{
    lark_media::{
        accept, lark, last, media_fixture, post, refusal, rendered, resource, scoped_mention,
        IMAGE_KEY,
    },
    support::*,
};
use cmaeg_core::activity::{ActivityKind, MediaVariant};
use serde_json::{json, Value};

const BOT: &str = "ou_bot";
const FLATTENED: &str =
    "Weekly update\nplease check the report (https://docs.example/r) with @Alice";

fn pinned(value: &mut Value) {
    scoped_mention(value);
    value["sources"][0]["bot_id"] = BOT.into();
}

/// A group post with a title, text, a link, the bot's and Alice's mentions and
/// an embedded image.
fn rich_post() -> Envelope {
    let content = json!({"title": "Weekly update", "content": [
        [{"tag": "at", "user_id": "@_user_1", "user_name": ""}, {"tag": "text", "text": " please check "},
         {"tag": "a", "text": "the report", "href": "https://docs.example/r"},
         {"tag": "text", "text": " with "}, {"tag": "at", "user_id": "@_user_2", "user_name": ""}],
        [{"tag": "img", "image_key": IMAGE_KEY}],
    ]});
    let mut envelope = lark("post", content, "unknown", Vec::new());
    envelope.group();
    envelope.lark_message()["mentions"] = json!([
        {"key": "@_user_1", "id": {"open_id": BOT, "union_id": "on_bot"}, "name": "Support Bot"},
        {"key": "@_user_2", "id": {"open_id": "ou_alice", "union_id": "on_alice"}, "name": "Alice"},
    ]);
    envelope
}

fn post_of(content: Value) -> Envelope {
    lark("post", content, "unknown", Vec::new())
}

#[tokio::test]
async fn lark_post_is_flattened_without_the_bot_mention() {
    let fixture = Fixture::on_with_media("lark", pinned, Default::default()).await;
    accept(&fixture, &rich_post(), "post").await;
    let (text, attachments) = last(&fixture);
    assert_eq!(text, FLATTENED);
    assert_eq!(attachments.len(), 1);
    let input = rendered(&fixture);
    assert!(
        input.starts_with(&format!("{FLATTENED}\n\n[Attachments: 1")),
        "{input}"
    );
    assert!(
        input.contains("\n[0] image embedded in the post · not retrievable through this channel"),
        "{input}"
    );
    for absent in ["Support Bot", "@_user_"] {
        assert!(!input.contains(absent), "{input}");
    }
}

#[tokio::test]
async fn lark_post_unpinned_bot_renders_every_mention() {
    let fixture = Fixture::on_with_media("lark", scoped_mention, Default::default()).await;
    accept(&fixture, &rich_post(), "unpinned").await;
    let expected = FLATTENED.replace("\nplease", "\n@Support Bot please");
    assert_eq!(last(&fixture).0, expected);
}

#[tokio::test]
async fn lark_post_over_32_kib_is_truncated_and_valid() {
    let fixture = media_fixture().await;
    let long = json!({"content": [[{"tag": "text", "text": "a".repeat(40 * 1_024)}]]});
    accept(&fixture, &post_of(long), "40 KiB post").await;
    let requests = fixture.recorder.requests.lock().unwrap();
    let activity = &requests[0].activity;
    activity.validate().unwrap();
    let ActivityKind::Message { text, .. } = &activity.kind else {
        panic!("expected a message");
    };
    assert!(text.len() <= 32 * 1_024, "{}", text.len());
    assert!(text.ends_with("\n[post truncated]"));
}

#[tokio::test]
async fn lark_post_with_17_images_lists_16() {
    let fixture = media_fixture().await;
    let images: Vec<Value> = (0..17)
        .map(|index| json!([{"tag": "img", "image_key": format!("img_v3_{index}")}]))
        .collect();
    accept(
        &fixture,
        &post_of(json!({ "content": images })),
        "17 images",
    )
    .await;
    let (text, attachments) = last(&fixture);
    assert_eq!(text, "[more embedded media omitted]");
    assert_eq!(attachments.len(), 16);
    assert_eq!(attachments[15].reference, "lark:img_v3_15");
    assert!(attachments
        .iter()
        .all(|item| item.source.is_none() && item.variant == Some(MediaVariant::Embedded)));
}

#[tokio::test]
async fn lark_empty_post_is_content_unsupported() {
    let fixture = media_fixture().await;
    let blank =
        json!({"title": " ", "content": [[{"tag": "text", "text": "\u{7}"}, {"tag": "hr_x"}]]});
    for content in [json!({"title": "", "content": []}), blank] {
        refusal(&fixture, &post_of(content), "content_unsupported").await;
    }
    assert!(fixture.recorder.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn lark_malformed_post_is_content_unsupported() {
    let fixture = media_fixture().await;
    let mut not_json = post();
    not_json.lark_message()["content"] = "not json".into();
    for envelope in [
        post_of(json!(["not an object"])),
        post_of(json!({"title": "t", "content": "not an array"})),
        post_of(json!({"content": [{"tag": "text", "text": "not a paragraph"}]})),
        not_json,
    ] {
        refusal(&fixture, &envelope, "content_unsupported").await;
    }
    assert!(fixture.recorder.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn lark_post_with_envelope_text_or_attachments_is_content_unsupported() {
    let fixture = media_fixture().await;
    let mut text = post();
    text.payload["content"]["text"] = "hello".into();
    let mut listed = post();
    listed.payload["content"]["attachments"] =
        json!([resource(0, "image_key", IMAGE_KEY, json!({}))]);
    let mut malformed = post();
    malformed.payload["content"]["attachments"] = json!(7);
    let mut typed = post();
    typed.payload["content"]["type"] = "text".into();
    for envelope in [text, listed, malformed, typed] {
        refusal(&fixture, &envelope, "content_unsupported").await;
    }
    assert!(fixture.recorder.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn lark_post_flag_off_is_content_unsupported() {
    let fixture = Fixture::on("lark", pinned, Default::default()).await;
    for envelope in [rich_post(), post()] {
        refusal(&fixture, &envelope, "content_unsupported").await;
    }
    assert!(fixture.recorder.requests.lock().unwrap().is_empty());
}

/// The status and the stable parts of the answer; a conversation ID depends on
/// the channel, which differs between fixtures.
async fn outcome(fixture: &Fixture, envelope: &Envelope) -> (u16, Value) {
    let response = fixture.send(envelope).await;
    let status = response.status().as_u16();
    let body: Value = response.json().await.unwrap();
    let stable = json!([
        body["status"],
        body["reason"],
        body["detail"],
        body["event_id"]
    ]);
    (status, stable)
}

fn mention(open_id: &str) -> Value {
    json!([{"key": "@_user_1", "id": {"open_id": open_id}, "name": "Someone"}])
}

fn group_text(envelope: &mut Envelope) {
    envelope.group();
    envelope.lark_text("@_user_1 status?");
}

/// The Lark text cases of `platforms.rs` and the outcome each earns: admitted,
/// unaddressed, malformed and refused messages.
fn text_fixtures() -> Vec<(Envelope, &'static str)> {
    let edits: [fn(&mut Envelope); 10] = [
        |_| {},
        group_text,
        |envelope| {
            group_text(envelope);
            envelope.lark_message()["mentions"] = mention(BOT);
        },
        |envelope| {
            group_text(envelope);
            envelope.lark_message()["mentions"] = mention("ou_other");
        },
        |envelope| {
            group_text(envelope);
            envelope.lark_message()["mentions"] = json!({"not": "a list"});
        },
        |envelope| envelope.lark_message()["content"] = "{\"text\":\"other\"}".into(),
        |envelope| envelope.lark_message()["content"] = "not json".into(),
        |envelope| envelope.lark_message()["chat_type"] = "topic_group".into(),
        |envelope| envelope.payload["content"]["type"] = "image".into(),
        |envelope| {
            envelope.payload["content"]["attachments"] =
                json!([resource(0, "image_key", IMAGE_KEY, json!({}))]);
        },
    ];
    let outcomes = [
        "accepted",
        "group_not_addressed",
        "accepted",
        "group_not_addressed",
        "lark_event_invalid",
        "content_unsupported",
        "content_unsupported",
        "system_event",
        "content_unsupported",
        "attachment_unsupported",
    ];
    edits
        .iter()
        .zip(outcomes)
        .map(|(edit, outcome)| {
            let mut envelope = Envelope::on("lark");
            edit(&mut envelope);
            (envelope, outcome)
        })
        .collect()
}

#[tokio::test]
async fn lark_text_messages_unchanged() {
    let off = Fixture::on("lark", pinned, Default::default()).await;
    let on = Fixture::on_with_media("lark", pinned, Default::default()).await;
    for (envelope, expected) in text_fixtures() {
        let (status, stable) = outcome(&on, &envelope).await;
        assert_eq!(outcome(&off, &envelope).await, (status, stable.clone()));
        // The status, refusal reason or problem detail names the outcome.
        assert!((0..3).any(|field| stable[field] == expected), "{stable}");
    }
    let activities = |fixture: &Fixture| -> Vec<_> {
        let requests = fixture.recorder.requests.lock().unwrap();
        requests
            .iter()
            .map(|request| request.activity.clone())
            .collect()
    };
    let admitted = activities(&on);
    assert_eq!(admitted.len(), 2);
    assert_eq!(activities(&off), admitted);
}
