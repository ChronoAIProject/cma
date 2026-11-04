use super::{
    content,
    forward::Snapshot,
    normalization_tests::raw,
    normalize::{normalize, normalize_with_projections, Normalized},
};
use crate::ingress::admission::Refusal;
use cmaeg_core::{
    activity::{ActivityKind, ActorKind},
    channel::AdmissionPolicy,
};
use serde_json::{json, Value};

pub(super) fn forwarded(raw: &mut Value) {
    raw["message_reference"] =
        json!({"type":1,"message_id":"123","channel_id":"456","guild_id":"789"});
    raw["message_snapshots"] = json!([{"message":{
        "type":0,"content":"Original words","attachments":[],"embeds":[],
        "timestamp":"2026-10-29T00:00:00Z","flags":0,"mentions":[],"mention_roles":[]
    }}]);
    raw["flags"] = (1 << 14).into();
}
fn projected(raw: &Value, policy: &AdmissionPolicy) -> Normalized {
    normalize_with_projections(raw, "77", policy, None, Snapshot::capture(raw).as_ref()).unwrap()
}

#[test]
fn forward_only_and_mixed_keep_forwarder_and_current_chat_without_origin_authority() {
    for mixed in [false, true] {
        let mut message = raw();
        forwarded(&mut message);
        message["content"] = if mixed { "My instruction" } else { "" }.into();
        message["message_snapshots"][0]["message"]["attachments"] = json!([
            {"id":"444","filename":"private.png","url":"http://127.0.0.1/private","content_type":"image/png"}
        ]);
        message["message_snapshots"][0]["message"]["embeds"] = json!([
            {"title":"Embedded title", "url":"https://private.example/secret"}
        ]);
        if mixed {
            message["attachments"] = json!([{"id":"500","filename":"outer.txt","size":4}]);
        }
        let Normalized::Activity(activity) = projected(&message, &AdmissionPolicy::All {}) else {
            panic!()
        };
        assert_eq!(activity.actor.id, "42");
        assert_eq!(activity.actor.kind, ActorKind::Human);
        assert_eq!(activity.conversation.id, "99");
        assert_eq!(activity.conversation.thread_id, None);
        let ActivityKind::Message {
            text,
            attachments,
            quoted_message,
            ..
        } = &activity.kind
        else {
            panic!()
        };
        assert!(text.contains("Original words"));
        assert!(text.contains("Original author is not provided by Discord"));
        assert!(text.starts_with(if mixed {
            "My instruction"
        } else {
            "\n\n[Discord"
        }));
        assert!(text.contains("\"attachments\":1"));
        assert!(text.contains("\"embeds\":1"));
        assert_eq!(attachments.len(), usize::from(mixed));
        assert!(quoted_message.is_none());
        let encoded = serde_json::to_string(&activity).unwrap();
        for private in ["127.0.0.1", "private.example", "private.png", "456", "789"] {
            assert!(!encoded.contains(private));
        }
    }
}

#[test]
fn supported_partial_message_types_and_media_only_snapshots_remain_descriptive() {
    for kind in [0, 19, 20, 23] {
        let mut message = raw();
        forwarded(&mut message);
        message["content"] = "".into();
        message["message_snapshots"][0]["message"]["type"] = kind.into();
        message["message_snapshots"][0]["message"]["content"] = "".into();
        message["message_snapshots"][0]["message"]["attachments"] =
            json!([{"url":"https://untrusted.example/file","id":"500"}]);
        let Normalized::Activity(activity) = projected(&message, &AdmissionPolicy::All {}) else {
            panic!()
        };
        assert!(
            matches!(&activity.kind, ActivityKind::Message { text, attachments, .. }
            if text.contains("\"attachments\":1") && attachments.is_empty())
        );
    }
}

#[test]
fn only_exact_single_forward_snapshots_add_content() {
    let mut original = raw();
    forwarded(&mut original);
    original["content"] = "".into();
    let mut cases = vec![];
    for bad in [json!(0), json!(2), json!("1"), Value::Null] {
        let mut value = original.clone();
        value["message_reference"]["type"] = bad;
        cases.push(value);
    }
    for key in ["message_id", "channel_id", "guild_id"] {
        let mut value = original.clone();
        value["message_reference"][key] = "001".into();
        cases.push(value);
    }
    for bad in [
        json!([]),
        json!([{}, {}]),
        json!({}),
        json!([{"message":null}]),
    ] {
        let mut value = original.clone();
        value["message_snapshots"] = bad;
        cases.push(value);
    }
    for key in [
        "author",
        "id",
        "channel_id",
        "guild_id",
        "message_snapshots",
        "message_reference",
        "poll",
        "call",
        "activity",
    ] {
        let mut value = original.clone();
        value["message_snapshots"][0]["message"][key] = json!({"id":"77"});
        cases.push(value);
    }
    for (key, bad) in [
        ("type", json!(6)),
        ("content", Value::Null),
        ("attachments", json!([{}])),
        ("embeds", json!("not-array")),
    ] {
        let mut value = original.clone();
        value["message_snapshots"][0]["message"][key] = bad;
        cases.push(value);
    }
    let mut empty = original.clone();
    empty["message_snapshots"][0]["message"]["content"] = "".into();
    cases.push(empty);
    for message in cases {
        assert!(Snapshot::capture(&message).is_none());
        assert!(!matches!(
            normalize_with_projections(&message, "77", &AdmissionPolicy::All {}, None, None),
            Ok(Normalized::Activity(_))
        ));
    }
    let mut spoof = original;
    spoof["referenced_message"] = json!({"id":"123","channel_id":"456","author":{"id":"77"}});
    assert!(Snapshot::capture(&spoof).is_none());
}

#[test]
fn snapshots_cannot_supply_bot_addressing_sender_permissions_or_thread_evidence() {
    let mut message = raw();
    forwarded(&mut message);
    message["content"] = "".into();
    message["guild_id"] = "88".into();
    message["message_snapshots"][0]["message"]["mentions"] = json!([{"id":"77"}]);
    message["message_snapshots"][0]["message"]["content"] = "<@77> instruction".into();
    for groups in ["mention", "reply_to_bot"] {
        let policy = serde_json::from_value(json!({"type":"scoped","senders":{"type":"open"},"chats":{"type":"open"},"groups":groups})).unwrap();
        assert!(matches!(
            projected(&message, &policy),
            Normalized::Refused(_)
        ));
    }
    assert!(matches!(
        projected(
            &message,
            &AdmissionPolicy::Allowlist {
                sender_ids: vec!["77".into()]
            }
        ),
        Normalized::Refused(Refusal::SenderDenied)
    ));
    for (key, value) in [
        ("bot", json!(true)),
        ("system", json!(true)),
        ("id", json!("77")),
    ] {
        let mut candidate = message.clone();
        candidate["author"][key] = value;
        assert!(matches!(
            projected(&candidate, &AdmissionPolicy::All {}),
            Normalized::Refused(Refusal::BotSender | Refusal::SystemSender)
        ));
    }
    message["webhook_id"] = "900".into();
    assert!(matches!(
        projected(&message, &AdmissionPolicy::All {}),
        Normalized::Refused(Refusal::BotSender)
    ));
}

#[test]
fn forwarded_text_bounds_and_omitted_media_never_create_nested_downloads() {
    let mut message = raw();
    forwarded(&mut message);
    message["content"] = "".into();
    message["message_snapshots"][0]["message"]["content"] = "界\u{202e}\"".repeat(20_000).into();
    message["message_snapshots"][0]["message"]["attachments"] =
        json!(vec![json!({"id":"500"}); 25]);
    let snapshot = Snapshot::capture(&message).unwrap();
    let text = snapshot.append(&message, String::new()).unwrap();
    assert!(text.len() <= 12 * 1024);
    assert!(text.contains("\"truncated\":true"));
    assert!(text.contains("\"attachments\":10"));
    assert!(!text.contains('\u{202e}'));
    let full = "x".repeat(32 * 1024);
    assert_eq!(snapshot.append(&message, full.clone()).unwrap(), full);
    for key in ["id", "channel_id"] {
        let mut changed = message.clone();
        changed[key] = "101".into();
        assert!(snapshot.verify(&changed).is_err());
    }
    for (key, value) in [
        ("author", json!({"id":"43"})),
        (
            "message_reference",
            json!({"type":1,"message_id":"999","channel_id":"456"}),
        ),
    ] {
        let mut changed = message.clone();
        changed[key] = value;
        assert!(snapshot.verify(&changed).is_err());
    }
    // Even a change outside the rendered/truncated prefix conflicts with custody.
    message["message_snapshots"][0]["message"]["attachments"][24]["url"] =
        "https://elsewhere.example".into();
    assert!(snapshot.verify(&message).is_err());
}

#[test]
fn legacy_and_rich_content_v1_do_not_gain_forward_projection() {
    let mut message = raw();
    forwarded(&mut message);
    let Normalized::Activity(activity) =
        normalize(&message, "77", &AdmissionPolicy::All {}).unwrap()
    else {
        panic!()
    };
    assert!(matches!(&activity.kind, ActivityKind::Message { text, .. } if text == "hello"));
    message["content"] = "".into();
    assert!(content::Snapshot::capture(&message).is_none());
    assert!(matches!(
        normalize(&message, "77", &AdmissionPolicy::All {}).unwrap(),
        Normalized::Refused(Refusal::ContentUnsupported)
    ));
    message["embeds"] = super::content_tests::embed();
    let rich = content::Snapshot::capture(&message).unwrap();
    let old = rich.text(&message).unwrap();
    assert!(!old.contains("Original words"));
    let forward = Snapshot::capture(&message).unwrap();
    let Normalized::Activity(activity) = normalize_with_projections(
        &message,
        "77",
        &AdmissionPolicy::All {},
        Some(&rich),
        Some(&forward),
    )
    .unwrap() else {
        panic!()
    };
    assert!(
        matches!(&activity.kind, ActivityKind::Message { text, .. } if text.starts_with(&old) && text.contains("Original words"))
    );
    let mut invalid = serde_json::to_value(forward).unwrap();
    invalid["version"] = 2.into();
    assert!(serde_json::from_value::<Snapshot>(invalid)
        .unwrap()
        .verify(&message)
        .is_err());
}
