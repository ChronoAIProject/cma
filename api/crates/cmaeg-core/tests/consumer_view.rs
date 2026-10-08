use cmaeg_core::{
    channel::{
        ChannelDefinition, Consumer, ConsumerReply, DefaultConsumer, EffectiveReply, PublishPolicy,
        ReplyChoice, Secret,
    },
    ids::{ConsumerId, ConsumerName, RuleId, DEFAULT_CONSUMER_ID},
};
use serde_json::{json, Value};
use std::collections::BTreeSet;

#[path = "routing_policy/fixture.rs"]
mod fixture;

fn definition() -> ChannelDefinition {
    let mut definition: ChannelDefinition =
        serde_json::from_str(include_str!("fixtures/channel.json")).unwrap();
    definition.secrets.provider_credential = Some(Secret::new("consumer-view-credential"));
    definition
}

#[test]
fn the_default_consumer_reads_the_top_level_fields() {
    let definition = definition();
    let default = ConsumerId::default_consumer();
    for view in [
        definition.consumer(None).unwrap(),
        definition.consumer(Some(&default)).unwrap(),
    ] {
        assert_eq!(view.id, &default);
        assert_eq!(view.name, DEFAULT_CONSUMER_ID);
        assert_eq!(view.label, DEFAULT_CONSUMER_ID);
        assert!(std::ptr::eq(view.profile, &definition.profile));
        assert_eq!(view.profile_digest, definition.profile_digest);
        assert!(std::ptr::eq(view.provider, &definition.provider));
        assert_eq!(view.agent_key_hash, definition.secrets.agent_key_hash);
        assert_eq!(
            view.credential.map(Secret::as_str),
            Some("consumer-view-credential")
        );
        assert!(view.mentionable);
        assert_eq!(view.non_human, None);
        assert_eq!(
            view.reply,
            EffectiveReply {
                auto_final: definition.reply.auto_final,
                destination: ReplyChoice::Origin,
            }
        );
        assert_eq!(view.publish, &PublishPolicy::default());
        assert!(!view.pending);
    }
}

#[test]
fn the_default_view_follows_the_stored_credential_and_reply_policy() {
    let mut definition = definition();
    definition.secrets.provider_credential = None;
    definition.reply.auto_final = !definition.reply.auto_final;
    let view = definition.consumer(None).unwrap();
    assert!(view.credential.is_none());
    assert_eq!(view.reply.auto_final, definition.reply.auto_final);
}

#[test]
fn effective_non_human_is_the_override_else_the_provider_slug() {
    let definition = definition();
    let mut view = definition.consumer(None).unwrap();
    let configured = BTreeSet::from(["cma_codex".to_owned()]);
    let other = BTreeSet::from(["nyxbot".to_owned()]);
    assert!(view.non_human_effective(&configured));
    assert!(!view.non_human_effective(&other));
    assert!(!view.non_human_effective(&BTreeSet::new()));
    view.non_human = Some(false);
    assert!(!view.non_human_effective(&configured));
    view.non_human = Some(true);
    assert!(view.non_human_effective(&BTreeSet::new()));
}

#[test]
fn a_channel_without_routing_has_only_the_default_consumer() {
    let definition = definition();
    let other = ConsumerId::parse("csm_0123456789ab").unwrap();
    assert!(definition.consumer(Some(&other)).is_none());
    let views: Vec<_> = definition.consumers().collect();
    assert_eq!(views.len(), 1);
    assert!(views[0].id.is_default());
}

#[test]
fn additional_consumers_read_their_own_binding_and_key() {
    let mut value = fixture::routed();
    value["routing"]["consumers"][1]["label"] = "Nyx Bot".into();
    value["routing"]["consumers"][1]["pending"] = true.into();
    value["routing"]["consumers"][1]["mentionable"] = false.into();
    value["routing"]["consumers"][1]["non_human"] = true.into();
    value["routing"]["consumers"][1]["reply"] =
        json!({"auto_final": false, "destination": "default"});
    value["routing"]["consumers"][1]["publish"] = json!({"enabled": true, "targets": ["default"]});
    let definition = fixture::definition(value);
    let reviewer = ConsumerId::parse(fixture::REVIEWER).unwrap();
    let view = definition.consumer(Some(&reviewer)).unwrap();
    let stored = &definition.routing.as_ref().unwrap().consumers[0];
    assert_eq!(view.id, &reviewer);
    assert_eq!((view.name, view.label), ("reviewer", "reviewer"));
    assert!(std::ptr::eq(view.profile, &stored.profile));
    assert!(std::ptr::eq(view.provider, &stored.provider));
    assert_eq!(view.profile_digest, stored.profile_digest);
    assert_eq!(view.agent_key_hash, stored.agent_key_hash);
    assert_eq!(
        view.credential.map(Secret::as_str),
        Some(fixture::agent_key(fixture::REVIEWER).as_str())
    );
    assert!(view.mentionable && !view.pending && view.non_human.is_none());
    assert_eq!(view.reply.auto_final, definition.reply.auto_final);

    let views: Vec<_> = definition.consumers().collect();
    let ids: Vec<_> = views.iter().map(|view| view.id.as_str()).collect();
    assert_eq!(ids, ["default", fixture::REVIEWER, fixture::NYX]);
    let nyx = views[2];
    assert_eq!((nyx.name, nyx.label), ("nyx", "Nyx Bot"));
    assert!(nyx.pending && !nyx.mentionable);
    assert_eq!(nyx.non_human, Some(true));
    assert!(nyx.non_human_effective(&BTreeSet::new()));
    assert_eq!(
        nyx.reply,
        EffectiveReply {
            auto_final: false,
            destination: ReplyChoice::Default,
        }
    );
    assert!(nyx.publish.enabled);
    assert_eq!(nyx.provider.slug.as_str(), "nyxbot");
    let unknown = ConsumerId::parse("csm_0123456789ab").unwrap();
    assert!(definition.consumer(Some(&unknown)).is_none());
}

#[test]
fn the_default_consumer_takes_its_presentation_from_routing() {
    let mut value = fixture::routed();
    let mut definition = fixture::definition(value.clone());
    let view = definition.consumer(None).unwrap();
    assert_eq!((view.name, view.label), ("lead", "lead"));
    assert!(std::ptr::eq(view.provider, &definition.provider));
    assert_eq!(
        view.credential.map(Secret::as_str),
        Some(fixture::agent_key("default").as_str())
    );
    value["routing"]["default_consumer"] = json!({"name": "lead", "label": "Lead",
        "mentionable": false, "non_human": false, "reply": {"auto_final": false},
        "publish": {"enabled": true}});
    definition = fixture::definition(value);
    let view = definition
        .consumer(Some(&ConsumerId::default_consumer()))
        .unwrap();
    assert!(view.id.is_default());
    assert_eq!(view.label, "Lead");
    assert!(!view.mentionable && !view.pending);
    assert_eq!(view.non_human, Some(false));
    assert_eq!(
        view.reply,
        EffectiveReply {
            auto_final: false,
            destination: ReplyChoice::Origin,
        }
    );
    assert!(view.publish.enabled && view.publish.targets.is_empty());
}

#[test]
fn a_consumer_without_a_stored_key_has_no_credential() {
    let mut value = fixture::routed();
    value["secrets"]["consumer_keys"] = json!([]);
    let definition = fixture::definition(value);
    let nyx = ConsumerId::parse(fixture::NYX).unwrap();
    assert!(definition
        .consumer(Some(&nyx))
        .unwrap()
        .credential
        .is_none());
}

#[test]
fn the_grammar_accepts_default_and_generated_ids() {
    for value in [DEFAULT_CONSUMER_ID, "csm_0123456789ab", "csm_abcdefabcdef"] {
        let id = ConsumerId::parse(value).unwrap();
        assert_eq!(id.as_str(), value);
        assert_eq!(id.to_string(), value);
        assert_eq!(id.is_default(), value == DEFAULT_CONSUMER_ID);
    }
    assert_eq!(ConsumerId::default_consumer().as_str(), "default");
}

#[test]
fn the_grammar_refuses_everything_else() {
    for value in [
        "csm_ABCDEF012345",
        "csm_0123456789a",
        "csm_0123456789abc",
        "csm_0123456789ag",
        "Default",
        "",
        "csm_",
        "csm_0123456789ab\x1f",
        " default",
    ] {
        let error = ConsumerId::parse(value).unwrap_err();
        assert_eq!(error.pointer, "/", "{value:?}");
    }
}

#[test]
fn ids_serialize_as_strings_and_deserialize_through_the_grammar() {
    let id = ConsumerId::parse("csm_0123456789ab").unwrap();
    assert_eq!(serde_json::to_string(&id).unwrap(), "\"csm_0123456789ab\"");
    let parsed: ConsumerId = serde_json::from_str("\"default\"").unwrap();
    assert!(parsed.is_default());
    assert!(serde_json::from_str::<ConsumerId>("\"csm_ABCDEF012345\"").is_err());
    assert!(serde_json::from_str::<ConsumerId>("7").is_err());
}

#[test]
fn consumer_names_accept_the_grammar_and_report_reserved_words() {
    for value in [
        "ab",
        "reviewer",
        "nyx-bot_2",
        &format!("a{}", "b".repeat(31)),
    ] {
        let name = ConsumerName::parse(value).unwrap();
        assert_eq!(name.as_str(), value);
        assert!(!name.is_reserved(), "{value}");
    }
    for value in ["all", "everyone", "here", "to", "csm_0123456789ab", "csm_x"] {
        assert!(ConsumerName::parse(value).unwrap().is_reserved(), "{value}");
    }
    let default = ConsumerName::parse("default").unwrap();
    assert!(default.is_default() && !default.is_reserved());
    for value in [
        "a",
        "",
        "Reviewer",
        "1st",
        "_x",
        "-x",
        "re viewer",
        "rev.iewer",
        "réviewer",
        &format!("a{}", "b".repeat(32)),
    ] {
        assert_eq!(
            ConsumerName::parse(value).unwrap_err().pointer,
            "/",
            "{value:?}"
        );
        assert!(serde_json::from_value::<ConsumerName>(json!(value)).is_err());
    }
}

#[test]
fn rule_ids_accept_only_their_grammar() {
    for value in ["a", "1", "lark-ops_2", &"r".repeat(32)] {
        assert_eq!(RuleId::parse(value).unwrap().to_string(), value);
    }
    for value in ["", "Rule", "rule.1", "rule 1", &"r".repeat(33)] {
        assert!(RuleId::parse(value).is_err(), "{value:?}");
        assert!(serde_json::from_value::<RuleId>(json!(value)).is_err());
    }
}

fn consumer_json() -> Value {
    let fixture: Value = serde_json::from_str(include_str!("fixtures/channel.json")).unwrap();
    json!({"id":"csm_0123456789ab","name":"reviewer","profile":fixture["profile"],
        "profile_digest":fixture["profile_digest"],"provider":fixture["provider"],
        "agent_key_hash":format!("sha256:{}", "1".repeat(64)),"created_at":"2026-10-01T00:00:00Z"})
}

#[test]
fn a_consumer_at_its_defaults_omits_every_optional_field() {
    let stored = consumer_json();
    let consumer: Consumer = serde_json::from_value(stored.clone()).unwrap();
    assert!(consumer.mentionable);
    assert_eq!(consumer.label, None);
    assert_eq!(consumer.non_human, None);
    assert!(consumer.reply.is_default());
    assert!(consumer.publish.is_disabled());
    assert!(!consumer.pending);
    assert_eq!(serde_json::to_value(&consumer).unwrap(), stored);
}

#[test]
fn a_consumer_keeps_every_optional_field_it_stores() {
    let mut stored = consumer_json();
    for (key, value) in [
        ("label", json!("Reviewer")),
        ("mentionable", json!(false)),
        ("non_human", json!(true)),
        ("reply", json!({"auto_final":false,"destination":"default"})),
        ("publish", json!({"enabled":true,"targets":["default"]})),
        ("pending", json!(true)),
    ] {
        stored[key] = value;
    }
    let consumer: Consumer = serde_json::from_value(stored.clone()).unwrap();
    assert_eq!(
        consumer.reply,
        ConsumerReply {
            auto_final: Some(false),
            destination: ReplyChoice::Default,
        }
    );
    assert_eq!(serde_json::to_value(&consumer).unwrap(), stored);
    // A disabled policy that still lists targets is not the default, so it is kept.
    stored["publish"] = json!({"enabled":false,"targets":["default"]});
    let consumer: Consumer = serde_json::from_value(stored.clone()).unwrap();
    assert_eq!(serde_json::to_value(&consumer).unwrap(), stored);
}

#[test]
fn consumer_shapes_refuse_unknown_fields_and_values() {
    let mut unknown = consumer_json();
    unknown["owner"] = json!("someone");
    assert!(serde_json::from_value::<Consumer>(unknown).is_err());
    let mut bad_id = consumer_json();
    bad_id["id"] = json!("csm_XYZ");
    assert!(serde_json::from_value::<Consumer>(bad_id).is_err());
    assert!(serde_json::from_value::<ConsumerReply>(json!({"destination":"elsewhere"})).is_err());
    assert!(serde_json::from_value::<PublishPolicy>(json!({"targets":[]})).is_err());
    assert!(
        serde_json::from_value::<DefaultConsumer>(json!({"name":"default","extra":1})).is_err()
    );
    let default: DefaultConsumer = serde_json::from_value(json!({"name":"default"})).unwrap();
    assert!(default.mentionable && default.reply.is_default() && default.publish.is_disabled());
    assert_eq!(
        serde_json::to_value(&default).unwrap(),
        json!({"name":"default"})
    );
}
