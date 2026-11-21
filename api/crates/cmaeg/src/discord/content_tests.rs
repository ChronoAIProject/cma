use super::{
    content::Snapshot,
    normalization_tests::raw,
    normalize::{normalize, normalize_with_content, Normalized},
};
use crate::ingress::admission::Refusal;
use cmaeg_core::{
    activity::{ActivityKind, ActorKind},
    channel::AdmissionPolicy,
};
use serde_json::{json, Value};

pub(super) fn sticker() -> Value {
    json!([{"id":"749054660769218631","name":"Wave","format_type":3}])
}
pub(super) fn embed() -> Value {
    json!([{"type":"article","title":"Project update","description":"Release is ready",
        "author":{"name":"Article writer"},"provider":{"name":"Example publication"},
        "fields":[{"name":"Version","value":"2"}],
        "image":{"url":"https://cdn.example/image.png?access=private"},
        "url":"https://example.com/article?access=private","footer":{"text":"Source footer"}}])
}
pub(super) fn poll() -> Value {
    json!({"question":{"text":"Which day?"},"answers":[
        {"answer_id":4,"poll_media":{"text":"Monday","emoji":{"name":"🌙"}}},
        {"answer_id":9,"poll_media":{"text":"Friday","emoji":{"id":"456"}}}],
        "expiry":"2026-10-30T00:00:00Z","allow_multiselect":true,"layout_type":1,
        "results":{"is_finalized":false,"answer_counts":[{"id":4,"count":123,"me_voted":true}]}})
}
fn projected(raw: &Value, policy: &AdmissionPolicy) -> Normalized {
    normalize_with_content(raw, "77", policy, Snapshot::capture(raw).as_ref()).unwrap()
}
fn text(activity: &cmaeg_core::activity::Activity) -> &str {
    let ActivityKind::Message { text, .. } = &activity.kind else {
        panic!()
    };
    text
}

#[test]
fn standalone_and_mixed_sticker_embed_poll_keep_human_attribution_without_download_authority() {
    for (key, value, expected) in [
        ("sticker_items", sticker(), "\"kind\":\"sticker\""),
        ("embeds", embed(), "\"kind\":\"embed\""),
        ("poll", poll(), "\"kind\":\"poll\""),
    ] {
        for mixed in [false, true] {
            let mut message = raw();
            message["content"] = if mixed { "My instruction" } else { "" }.into();
            message[key] = value.clone();
            if mixed {
                message["attachments"] = json!([{"id":"500","filename":"note.txt","size":4}]);
            }
            let Normalized::Activity(activity) = projected(&message, &AdmissionPolicy::All {})
            else {
                panic!()
            };
            assert_eq!(activity.actor.id, "42");
            assert_eq!(activity.actor.kind, ActorKind::Human);
            assert_eq!(activity.conversation.id, "99");
            assert_eq!(activity.event_id.as_str(), "discord:77:700");
            assert!(text(&activity).contains(expected));
            assert!(text(&activity).contains("sender 42 on message 700"));
            assert_eq!(text(&activity).starts_with("My instruction"), mixed);
            let serialized = serde_json::to_string(&activity).unwrap();
            assert!(!serialized.contains("access=private"));
            assert!(!serialized.contains("me_voted"));
            assert!(!serialized.contains("answer_counts"));
            let ActivityKind::Message { attachments, .. } = &activity.kind else {
                panic!()
            };
            assert_eq!(attachments.len(), usize::from(mixed));
            if key == "embeds" {
                assert!(text(&activity).contains("Article writer"));
                assert!(text(&activity).contains("Example publication"));
            }
        }
    }
}

#[test]
fn content_cannot_override_bot_webhook_system_sender_chat_or_mention_policy() {
    for (key, value) in [
        ("sticker_items", sticker()),
        ("embeds", embed()),
        ("poll", poll()),
    ] {
        let mut message = raw();
        message["content"] = "".into();
        message[key] = value;
        for defect in 0..4 {
            let mut candidate = message.clone();
            match defect {
                0 => candidate["author"]["bot"] = true.into(),
                1 => candidate["author"]["id"] = "77".into(),
                2 => candidate["webhook_id"] = "800".into(),
                _ => candidate["author"]["system"] = true.into(),
            }
            assert!(matches!(
                projected(&candidate, &AdmissionPolicy::All {}),
                Normalized::Refused(Refusal::BotSender | Refusal::SystemSender)
            ));
        }
        assert!(matches!(
            projected(
                &message,
                &AdmissionPolicy::Allowlist {
                    sender_ids: vec!["43".into()]
                }
            ),
            Normalized::Refused(Refusal::SenderDenied)
        ));
        message["guild_id"] = "88".into();
        let mut policy: Value = json!({"type":"scoped","senders":{"type":"open"},"chats":{"type":"open"},"groups":"mention"});
        assert!(matches!(
            projected(&message, &serde_json::from_value(policy.clone()).unwrap()),
            Normalized::Refused(Refusal::GroupNotAddressed)
        ));
        message["mentions"] = json!([{"id":"77"}]);
        policy["chats"] = json!({"type":"deny"});
        assert!(matches!(
            projected(&message, &serde_json::from_value(policy).unwrap()),
            Normalized::Refused(Refusal::ChatDenied)
        ));
    }
}

#[test]
fn empty_malformed_or_unknown_metadata_never_creates_synthetic_content() {
    for (key, value) in [
        (
            "embeds",
            json!([{}, {"type":"rich"}, {"author":{"name":"77"}}, {"url":"javascript:alert(1)"}]),
        ),
        ("embeds", json!("not-an-array")),
        (
            "sticker_items",
            json!([{"id":"1","name":"","format_type":1}]),
        ),
        (
            "sticker_items",
            json!([{"id":"001","name":"Wave","format_type":1}]),
        ),
        (
            "sticker_items",
            json!([{"id":"1","name":"Wave","format_type":99}]),
        ),
        ("poll", json!({})),
        (
            "poll",
            json!({"question":{"text":"Question?"},"answers":[]}),
        ),
        (
            "message_snapshots",
            json!([{"message":{"content":"not current sender content"}}]),
        ),
    ] {
        let mut message = raw();
        message["content"] = "".into();
        message[key] = value;
        assert!(Snapshot::capture(&message).is_none());
        assert!(matches!(
            projected(&message, &AdmissionPolicy::All {}),
            Normalized::Refused(Refusal::ContentUnsupported)
        ));
    }
    let mut message = raw();
    message["poll"] = poll();
    message["poll"]["answers"][1]["answer_id"] = 4.into();
    assert!(Snapshot::capture(&message).is_none());
    message["poll"] = poll();
    message["poll"]["layout_type"] = 99.into();
    assert!(Snapshot::capture(&message).is_none());
}

#[test]
fn projection_is_bounded_preserves_original_text_and_rejects_cross_message_snapshots() {
    let mut message = raw();
    message["content"] = "".into();
    let mut huge = embed()[0].clone();
    huge["description"] = "界\n\"".repeat(20_000).into();
    huge["fields"] = Value::Array(vec![json!({"name":"n","value":"v"}); 25]);
    message["embeds"] = Value::Array(vec![huge; 30]);
    let snapshot = Snapshot::capture(&message).unwrap();
    let projected = snapshot.text(&message).unwrap();
    assert!(projected.len() <= 16 * 1024);
    assert!(projected.contains("\"truncated\":true"));
    assert!(projected.contains("Additional metadata omitted"));
    message["content"] = "x".repeat(32 * 1024).into();
    assert_eq!(
        snapshot.text(&message).unwrap(),
        message["content"].as_str().unwrap()
    );
    for key in ["id", "channel_id"] {
        let mut changed = message.clone();
        changed[key] = "123".into();
        assert!(snapshot.text(&changed).is_err());
    }
    message["author"]["id"] = "43".into();
    assert!(snapshot.text(&message).is_err());
}

#[test]
fn legacy_normalization_never_inherits_content_projection() {
    let mut message = raw();
    message["sticker_items"] = sticker();
    let Normalized::Activity(activity) =
        normalize(&message, "77", &AdmissionPolicy::All {}).unwrap()
    else {
        panic!()
    };
    assert_eq!(text(&activity), "hello");
    message["content"] = "".into();
    assert!(matches!(
        normalize(&message, "77", &AdmissionPolicy::All {}).unwrap(),
        Normalized::Refused(Refusal::ContentUnsupported)
    ));
}

#[test]
fn poll_keeps_emoji_only_nonsequential_answers_and_unknown_results_without_inventing_votes() {
    let mut message = raw();
    message["content"] = "".into();
    message["poll"] = poll();
    message["poll"]["expiry"] = Value::Null;
    message["poll"]["answers"][0]["poll_media"] = json!({"emoji":{"name":"🌙"}});
    message["poll"]["answers"][1]["poll_media"] = json!({"emoji":{"id":"456"}});
    message["poll"].as_object_mut().unwrap().remove("results");
    let snapshot = Snapshot::capture(&message).unwrap();
    let projected = snapshot.text(&message).unwrap();
    assert!(projected.contains("\"id\":4"));
    assert!(projected.contains("\"id\":9"));
    assert!(projected.contains("\"emoji_id\":\"456\""));
    assert!(projected.contains("\"expires_at\":null"));
    assert!(!projected.contains("answer_counts"));
    let mut corrupt = serde_json::to_value(snapshot).unwrap();
    corrupt["facts"][0]["question"] = "tampered".into();
    let corrupt: Snapshot = serde_json::from_value(corrupt).unwrap();
    assert!(corrupt.text(&message).is_err());
}
