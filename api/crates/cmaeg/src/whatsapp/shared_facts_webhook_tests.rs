use super::{
    shared_facts::tests::{contact, place},
    tests::{envelope, source, text},
    webhook::{self, Content, Error},
};
use cmaeg_core::channel::Source;
use serde_json::json;
use sha2::{Digest, Sha256};

#[test]
fn native_shared_facts_deduplicate_bounded_projections_and_preserve_legacy_hashes() {
    for raw in [contact("wamid.shared"), place("wamid.shared")] {
        let messages = webhook::extract(
            &source(),
            &envelope(vec![
                raw.clone(),
                raw.clone(),
                text("wamid.text", "ordinary"),
            ]),
        )
        .unwrap();
        assert_eq!(messages.len(), 2);
        assert!(
            matches!(&messages[0].content, Content::Text { text } if text.starts_with("Shared "))
        );
        let mut changed = raw.clone();
        if raw["type"] == "contacts" {
            changed["contacts"][0]["name"]["formatted_name"] = "Different name".into();
        } else {
            changed["location"]["longitude"] = 104.into();
        }
        assert_eq!(
            webhook::extract(&source(), &envelope(vec![raw, changed])),
            Err(Error::Message)
        );
    }
    let original = webhook::extract(&source(), &envelope(vec![text("wamid.original", "hello")]))
        .unwrap()
        .remove(0);
    let bytes = serde_json::to_vec(&original).unwrap();
    assert_eq!(bytes, br#"{"id":"wamid.original","sender":"447700900123","timestamp":1800000000,"content":{"kind":"text","text":"hello"}}"#);
    assert_eq!(
        format!("{:x}", Sha256::digest(&bytes)),
        "903945ce2761d19b744584bf4d8889d0e9dac58d20d22495cba3aaa97097a52d"
    );
    assert_eq!(
        webhook::event_id(&source(), "wamid.original")
            .unwrap()
            .as_str(),
        "whatsapp:04c3f9b2761f579cfc8a6d846e69dc68e79d906112c3035f43f83f9c8bd80b9d"
    );
}

#[test]
fn native_shared_facts_exclude_even_owner_selected_groups_and_reject_private_identity_spoofing() {
    let mut group_source = source();
    let Source::Whatsapp { group_ids, .. } = &mut group_source else {
        panic!()
    };
    group_ids.push("group_A+/==".into());
    for raw in [
        contact("wamid.group-contact"),
        place("wamid.group-location"),
    ] {
        let mut group = raw.clone();
        group["group_id"] = "group_A+/==".into();
        assert!(webhook::extract(&group_source, &envelope(vec![group]))
            .unwrap()
            .is_empty());
        for field in ["group", "participant", "recipient_type"] {
            let mut spoofed = raw.clone();
            spoofed[field] = "group".into();
            assert_eq!(
                webhook::extract(&source(), &envelope(vec![spoofed])),
                Err(Error::Message)
            );
        }
        let mut invalid = raw.clone();
        invalid["from"] = json!({"wa_id":"999999"});
        assert_eq!(
            webhook::extract(&source(), &envelope(vec![invalid])),
            Err(Error::Message)
        );
    }
}

#[test]
fn native_shared_facts_cannot_alias_original_text_or_message_change_authority() {
    for raw in [contact("wamid.shared"), place("wamid.shared")] {
        let parsed = webhook::extract(&source(), &envelope(vec![raw.clone()]))
            .unwrap()
            .remove(0);
        let Content::Text { text: display } = parsed.content else {
            panic!()
        };
        let text = text("wamid.shared", &display);
        for messages in [vec![raw.clone(), text.clone()], vec![text, raw.clone()]] {
            assert_eq!(
                webhook::extract(&source(), &envelope(messages)),
                Err(Error::Message)
            );
        }
        assert!(webhook::change_originals(&source(), &envelope(vec![raw]))
            .unwrap()
            .is_empty());
    }
}
