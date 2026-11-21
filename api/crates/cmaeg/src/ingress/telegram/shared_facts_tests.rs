use super::extract;
use crate::ingress::{
    admission::Refusal,
    relay::{facts::Observation, telegram_media::MediaIngress},
    telegram::{self, Outcome},
};
use cmaeg_core::activity::ActorKind;
use serde_json::{json, Value};

fn update(content: Value) -> Value {
    let mut message = json!({
        "message_id": 42, "date": 1700000000,
        "from": {"id": 91, "is_bot": false, "first_name": "Sender"},
        "chat": {"id": 91, "type": "private"}
    });
    message
        .as_object_mut()
        .unwrap()
        .extend(content.as_object().unwrap().clone());
    json!({"update_id": 71, "message": message})
}

fn admitted(content: Value) -> telegram::Observed {
    match telegram::observe(update(content), "500", Some("sample_bot"), false, false).unwrap() {
        Outcome::Admitted(message) => *message,
        Outcome::Refused(_) => panic!("expected shared display facts"),
    }
}

#[test]
fn contact_display_does_not_replace_authenticated_sender_or_grant_resource_authority() {
    let observed = admitted(json!({"contact": {
        "first_name": "Recipient", "last_name": "Example", "phone_number": "+65 1234 5678",
        "user_id": 999, "vcard": "https://internal.example/never-fetch"
    }}));
    let text = observed.payload.content.text.as_deref().unwrap();
    assert!(text.contains("Recipient") && text.contains("+65 1234 5678"));
    assert!(!text.contains("999") && !text.contains("never-fetch"));
    assert_eq!(observed.payload.sender.platform_id, "91");
    assert_eq!(observed.payload.conversation.platform_id, "91");
    assert_eq!(observed.facts.actor, ActorKind::Human);
    assert!(observed.facts.attachments.is_empty());
    assert!(observed.facts.event.is_none());
}

#[test]
fn location_and_venue_work_without_the_media_download_gate() {
    for (content, expected) in [
        (
            json!({"location":{"latitude":1.3,"longitude":103.8,"horizontal_accuracy":5}}),
            "Shared location",
        ),
        (
            json!({"venue":{"title":"Office","address":"One Street","location":{"latitude":1.3,"longitude":103.8},"google_place_id":"not-authority"}}),
            "Shared venue",
        ),
    ] {
        let observed = admitted(content);
        let text = observed.payload.content.text.unwrap();
        assert!(text.contains(expected) && text.contains("103.8"));
        assert!(!text.contains("not-authority"));
        assert!(observed.facts.attachments.is_empty());
    }
}

#[test]
fn live_location_is_honestly_a_snapshot_and_venue_location_must_agree() {
    let location = json!({"latitude":-90,"longitude":180,"live_period":900,"heading":180,"proximity_alert_radius":25});
    assert!(extract(&json!({"location":location}))
        .unwrap()
        .unwrap()
        .contains("not a continuous"));
    let location = json!({"latitude":-90,"longitude":180});
    let mut value =
        json!({"location": location, "venue":{"title":"A","address":"B","location":location}});
    assert!(extract(&value)
        .unwrap()
        .unwrap()
        .starts_with("Shared venue"));
    value["location"]["latitude"] = 0.into();
    assert!(extract(&value).is_err());
}

#[test]
fn malformed_or_ambiguous_shared_messages_are_refused_without_fabricated_text() {
    let examples = [
        json!({"contact":null}),
        json!({"contact":{"first_name":"X","phone_number":"not-a-number"}}),
        json!({"contact":{"first_name":"X\u{0000}","phone_number":"123"}}),
        json!({"contact":{"first_name":"x".repeat(257),"phone_number":"123"}}),
        json!({"location":{"latitude":91,"longitude":0}}),
        json!({"location":{"latitude":"1","longitude":0}}),
        json!({"location":{"latitude":0,"longitude":181}}),
        json!({"location":{"latitude":0,"longitude":0,"horizontal_accuracy":-1}}),
        json!({"location":{"latitude":0,"longitude":0},"text":"forged text"}),
        json!({"location":{"latitude":0,"longitude":0},"photo":[]}),
        json!({"location":{"latitude":0,"longitude":0},"contact":{"first_name":"X","phone_number":"123"}}),
        json!({"venue":{"title":"X","location":{"latitude":0,"longitude":0}}}),
        json!({"venue":{"title":"X","address":"Y","location":{"latitude":0,"longitude":0,"live_period":100}}}),
        json!({"location":{"latitude":0,"longitude":0,"heading":361}}),
        json!({"contact":{"first_name":"Name\nPhone: forged","phone_number":"123"}}),
    ];
    for content in examples {
        assert!(extract(&content).is_err(), "{content}");
        assert!(matches!(
            telegram::observe(update(content), "500", None, false, false).unwrap(),
            Outcome::Refused(Refusal::ContentUnsupported)
        ));
    }
}

#[test]
fn shared_contact_name_is_escaped_and_does_not_authorize_bot_mention() {
    let observed =
        admitted(json!({"contact":{"first_name":"@sample_bot \"quoted\"","phone_number":"123"}}));
    assert!(observed
        .payload
        .content
        .text
        .unwrap()
        .contains("\\\"quoted\\\""));
    assert_eq!(observed.facts.mentioned, Some(false));
}

#[test]
fn group_topic_and_sender_policy_facts_are_retained() {
    let mut raw = update(json!({"location":{"latitude":1,"longitude":2}}));
    raw["message"]["chat"] = json!({"id":-1001,"type":"supergroup"});
    raw["message"]["message_thread_id"] = 7.into();
    raw["message"]["is_topic_message"] = true.into();
    let Outcome::Admitted(observed) =
        telegram::observe(raw.clone(), "500", None, false, false).unwrap()
    else {
        panic!()
    };
    assert_eq!(observed.facts.thread_id.as_deref(), Some("7"));
    assert_eq!(observed.payload.sender.platform_id, "91");
    raw["message"]["from"]["is_bot"] = true.into();
    assert!(matches!(
        telegram::observe(raw, "500", None, false, false).unwrap(),
        Outcome::Refused(Refusal::BotSender)
    ));
}

#[test]
fn legacy_relay_does_not_gain_native_shared_fact_authority() {
    let observed = admitted(json!({"location":{"latitude":1,"longitude":2}}));
    assert!(matches!(
        crate::ingress::relay::telegram::observe(
            &observed.payload,
            None,
            Some("500"),
            false,
            MediaIngress::Off
        )
        .unwrap(),
        Observation::Refused(Refusal::ContentUnsupported)
    ));
}

#[test]
fn channel_shared_location_keeps_unknown_actor_and_explicit_opt_in() {
    let raw = json!({"update_id":72,"channel_post":{"message_id":42,"date":1700000000,"chat":{"id":-1001,"type":"channel","title":"Channel"},"location":{"latitude":1,"longitude":2}}});
    let policy = cmaeg_core::channel::AdmissionPolicy::All {};
    assert!(matches!(
        telegram::observe_source(raw.clone(), "500", None, false, &policy, false).unwrap(),
        Outcome::Refused(Refusal::SystemSender)
    ));
    let Outcome::Admitted(observed) =
        telegram::observe_source(raw, "500", None, false, &policy, true).unwrap()
    else {
        panic!()
    };
    assert_eq!(observed.facts.actor, ActorKind::Unknown);
    assert!(observed
        .payload
        .content
        .text
        .unwrap()
        .starts_with("Shared location"));
}
