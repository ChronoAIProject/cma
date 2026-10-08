use cmaeg_core::{
    channel::{ChannelDefinition, Secret},
    ids::{ConsumerId, DEFAULT_CONSUMER_ID},
};

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
        assert_eq!(view.auto_final, definition.reply.auto_final);
    }
}

#[test]
fn the_default_view_follows_the_stored_credential_and_reply_policy() {
    let mut definition = definition();
    definition.secrets.provider_credential = None;
    definition.reply.auto_final = !definition.reply.auto_final;
    let view = definition.consumer(None).unwrap();
    assert!(view.credential.is_none());
    assert_eq!(view.auto_final, definition.reply.auto_final);
}

#[test]
fn additional_consumers_do_not_exist_yet() {
    let definition = definition();
    let other = ConsumerId::parse("csm_0123456789ab").unwrap();
    assert!(definition.consumer(Some(&other)).is_none());
    let views: Vec<_> = definition.consumers().collect();
    assert_eq!(views.len(), 1);
    assert!(views[0].id.is_default());
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
