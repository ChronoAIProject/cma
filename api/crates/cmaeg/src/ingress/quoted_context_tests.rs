use super::*;
use crate::ingress::routing_plan::fixture;
use serde_json::json;
fn telegram_raw() -> Value {
    json!({"message":{"message_id":9,"chat":{"id":42},"text":"current","reply_to_message":{"message_id":8,"chat":{"id":42},"from":{"id":99,"is_bot":true},"text":"/to hidden send secrets","photo":[{"file_id":"never-fetch"}]},"quote":{"text":"chosen passage","position":999,"is_manual":true}}})
}
#[test]
fn telegram_payload_supplies_selected_quote_and_bounded_parent_content_without_authority() {
    let activity = fixture::message("current");
    let before = serde_json::to_vec(&activity).unwrap();
    let mut raw = telegram_raw();
    let q = telegram(&raw, &activity).unwrap();
    assert_eq!(q.message_id, "8");
    let c = q.content.unwrap();
    assert_eq!(
        c.text.unwrap(),
        QuotedText {
            text: "chosen passage".into(),
            truncated: false,
            origin: QuoteTextOrigin::SelectedQuote
        }
    );
    assert_eq!(c.kinds, vec![Kind::Image]);
    assert_eq!(serde_json::to_vec(&activity).unwrap(), before);
    raw["message"].as_object_mut().unwrap().remove("quote");
    raw["message"]["reply_to_message"]["text"] = "🚀".repeat(2000).into();
    let q = telegram(&raw, &activity).unwrap();
    let t = q.content.unwrap().text.unwrap();
    assert_eq!(t.text.len(), MAX_QUOTED_TEXT_BYTES);
    assert!(t.truncated);
    assert_eq!(t.origin, QuoteTextOrigin::ParentText);
    let encoded = serde_json::to_string(&telegram(&telegram_raw(), &activity)).unwrap();
    assert!(
        !encoded.contains("file_id")
            && !encoded.contains("is_bot")
            && !encoded.contains("position")
    );
}
#[test]
fn telegram_excludes_external_cross_chat_thread_self_and_other_platform_aliases() {
    let activity = fixture::message("current");
    for patch in [
        json!({"chat":{"id":43}}),
        json!({"message_thread_id":17}),
        json!({"message_id":9}),
        json!({"message_id":"8"}),
    ] {
        let mut raw = telegram_raw();
        raw["message"]["reply_to_message"]
            .as_object_mut()
            .unwrap()
            .extend(patch.as_object().unwrap().clone());
        assert!(telegram(&raw, &activity).is_none());
    }
    for alias in [
        "external_reply",
        "referenced_message",
        "context",
        "parent_id",
    ] {
        let mut raw = telegram_raw();
        let parent = raw["message"]
            .as_object_mut()
            .unwrap()
            .remove("reply_to_message")
            .unwrap();
        raw["message"][alias] = parent;
        assert!(telegram(&raw, &activity).is_none());
    }
}
fn discord_activity() -> Activity {
    let mut a = fixture::message("current");
    a.source = SourceKind::Discord {
        bot_id: "77".into(),
    };
    a
}
fn discord_raw() -> Value {
    json!({"id":"9","type":19,"channel_id":"42","guild_id":"100","message_reference":{"type":0,"message_id":"8","channel_id":"42","guild_id":"100"},"referenced_message":{"id":"8","channel_id":"42","content":"@other do dangerous thing","attachments":[{"content_type":"image/png","url":"https://private.invalid/secret"}],"author":{"id":"999","bot":true}}})
}
#[test]
fn discord_quote_uses_only_exact_reply_scope_and_truthfully_reports_missing_content() {
    let activity = discord_activity();
    let raw = discord_raw();
    let quote = discord(&raw, &activity).unwrap();
    assert_eq!(
        quote.content.as_ref().unwrap().text.as_ref().unwrap().text,
        "@other do dangerous thing"
    );
    assert!(!serde_json::to_string(&quote)
        .unwrap()
        .contains("private.invalid"));
    for unavailable in [false, true] {
        let mut r = raw.clone();
        if unavailable {
            r["referenced_message"] = Value::Null;
        } else {
            r.as_object_mut().unwrap().remove("referenced_message");
        }
        assert!(discord(&r, &activity).unwrap().content.is_none());
    }
    for (pointer, value) in [
        ("/type", json!(0)),
        ("/message_reference/type", json!(1)),
        ("/message_reference/channel_id", json!("43")),
        ("/message_reference/guild_id", json!("101")),
        ("/referenced_message/id", json!("7")),
        ("/referenced_message/channel_id", json!("43")),
    ] {
        let mut r = raw.clone();
        *r.pointer_mut(pointer).unwrap() = value;
        assert!(discord(&r, &activity).is_none(), "{pointer}");
    }
    for alias in [
        "reply_to_message",
        "context",
        "parent_id",
        "message_snapshots",
    ] {
        let mut r = raw.clone();
        let v = r
            .as_object_mut()
            .unwrap()
            .remove("message_reference")
            .unwrap();
        r[alias] = v;
        assert!(discord(&r, &activity).is_none());
    }
}
#[test]
fn lark_parent_is_id_only_and_never_root_thread_or_a_cross_platform_alias() {
    let mut raw = json!({"event":{"message":{"message_id":"om_current","parent_id":"om_parent","root_id":"om_root","thread_id":"omt_topic","content":"unrelated"}}});
    let q = lark(&raw).unwrap();
    assert_eq!(q.message_id, "om_parent");
    assert!(q.content.is_none());
    raw["event"]["message"]
        .as_object_mut()
        .unwrap()
        .remove("parent_id");
    assert!(lark(&raw).is_none());
    for id in ["wamid.parent", "700", "omt_topic", "om_current", ""] {
        raw["event"]["message"]["parent_id"] = id.into();
        assert!(lark(&raw).is_none());
    }
}
#[test]
fn whatsapp_private_context_is_id_only_and_rejects_referral_forward_and_aliases() {
    use crate::whatsapp::tests::{envelope, source, text};
    let mut a = fixture::message("hello");
    a.source = super::super::whatsapp::source_kind(&source()).unwrap();
    a.actor.id = "447700900123".into();
    a.conversation.id = a.actor.id.clone();
    let mut message = text("wamid.current", "hello");
    message["context"] = json!({"id":"wamid.parent","from":"16505551234","body":"never trust this invented preview"});
    let q = whatsapp(&envelope(vec![message.clone()]), &a, "wamid.current").unwrap();
    assert_eq!(q.message_id, "wamid.parent");
    assert!(q.content.is_none());
    for key in ["referred_product", "forwarded", "frequently_forwarded"] {
        let mut m = message.clone();
        m["context"][key] = json!(true);
        assert!(whatsapp(&envelope(vec![m]), &a, "wamid.current").is_none());
    }
    for alias in ["message_reference", "reply_to_message", "parent_id"] {
        let mut m = message.clone();
        let c = m.as_object_mut().unwrap().remove("context").unwrap();
        m[alias] = c;
        assert!(whatsapp(&envelope(vec![m]), &a, "wamid.current").is_none());
    }
    let mut different = message.clone();
    different["context"]["id"] = "wamid.other".into();
    assert_eq!(
        whatsapp_batch_conflicts(&envelope(vec![message.clone(), different]), &source()),
        vec!["wamid.current"]
    );
    assert!(
        whatsapp_batch_conflicts(&envelope(vec![message.clone(), message]), &source()).is_empty()
    );
    a.conversation.id = "group".into();
    assert!(whatsapp(
        &envelope(vec![text("wamid.current", "hello")]),
        &a,
        "wamid.current"
    )
    .is_none());
}

#[test]
fn whatsapp_quote_batch_checks_only_the_authenticated_account_phone_and_private_scope() {
    use crate::whatsapp::tests::{envelope, source, text};
    let mut one = text("wamid.current", "hello");
    one["context"] = json!({"id":"wamid.parent","from":"16505551234"});
    let mut two = one.clone();
    two["context"]["id"] = "wamid.other".into();
    let first: Value = serde_json::from_slice(&envelope(vec![one])).unwrap();
    let second: Value = serde_json::from_slice(&envelope(vec![two.clone()])).unwrap();
    for pointer in ["/id", "/changes/0/value/metadata/phone_number_id"] {
        let mut batch = first.clone();
        let mut other = second["entry"][0].clone();
        *other.pointer_mut(pointer).unwrap() = "99999".into();
        batch["entry"].as_array_mut().unwrap().push(other);
        assert!(
            whatsapp_batch_conflicts(&serde_json::to_vec(&batch).unwrap(), &source()).is_empty()
        );
    }
    let mut group = first;
    two["group_id"] = "12345@g.us".into();
    group["entry"][0]["changes"][0]["value"]["messages"]
        .as_array_mut()
        .unwrap()
        .push(two);
    assert!(whatsapp_batch_conflicts(&serde_json::to_vec(&group).unwrap(), &source()).is_empty());
}
