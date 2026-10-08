//! Restoring a routed channel: its additional consumers are checked as the
//! default one is, and a stored definition that fails is never served.
use super::{routing_fixture, support::Fixture};
use cmaeg::{channels::resolver::ResolveError, ChannelCacheConfig};
use cmaeg_core::{
    channel::{ChannelDefinition, Secret},
    ids::ConsumerId,
};

async fn routed() -> (Fixture, ChannelDefinition) {
    let fixture = Fixture::new(ChannelCacheConfig::default()).await;
    let mut definition = routing_fixture::definition(routing_fixture::routed());
    definition.record_id = fixture.definition.record_id.clone();
    (fixture, definition)
}

async fn restore(mut fixture: Fixture, definition: ChannelDefinition) -> Result<(), ResolveError> {
    fixture.definition = definition;
    // The resolver reports the record's version as the definition's.
    fixture.definition.version = 2;
    fixture.store(2);
    let result = fixture.resolver.resolve(fixture.id.as_str()).await;
    fixture.server.stop().await;
    result.map(|channel| assert_eq!(channel.definition, fixture.definition))
}

#[tokio::test]
async fn a_consistent_routed_channel_restores_with_every_consumer_key_hash() {
    let (mut fixture, mut definition) = routed().await;
    definition.version = 2;
    fixture.definition = definition;
    fixture.store(2);
    let channel = fixture.resolver.resolve(fixture.id.as_str()).await.unwrap();
    fixture.server.stop().await;
    assert_eq!(channel.definition, fixture.definition);
    let decoded = |consumer: &str| {
        let hex = routing_fixture::hash(&routing_fixture::agent_key(consumer));
        let digits = hex.strip_prefix("sha256:").unwrap();
        let bytes: Vec<u8> = (0..32)
            .map(|index| u8::from_str_radix(&digits[index * 2..index * 2 + 2], 16).unwrap())
            .collect();
        <[u8; 32]>::try_from(bytes).unwrap()
    };
    let expected: Vec<(String, [u8; 32])> =
        ["default", routing_fixture::REVIEWER, routing_fixture::NYX]
            .into_iter()
            .map(|consumer| (consumer.to_owned(), decoded(consumer)))
            .collect();
    let actual: Vec<(String, [u8; 32])> = channel
        .consumer_keys
        .iter()
        .map(|(id, hash)| (id.to_string(), *hash))
        .collect();
    assert_eq!(actual, expected);
    assert_eq!(channel.agent_key_hash, decoded("default"));
    let reviewer = ConsumerId::parse(routing_fixture::REVIEWER).unwrap();
    assert_eq!(
        channel.consumer_key_hash(Some(&reviewer)),
        Some(&decoded(routing_fixture::REVIEWER))
    );
}

#[tokio::test]
async fn a_consumer_key_that_does_not_match_its_hash_is_refused() {
    let (fixture, mut definition) = routed().await;
    definition.secrets.consumer_keys[1].1 = Secret::new("nyx_fixture_another_key");
    definition.validate().unwrap();
    assert_eq!(
        restore(fixture, definition).await,
        Err(ResolveError::InvalidDefinition)
    );
}

#[tokio::test]
async fn a_consumer_profile_that_does_not_match_its_digest_is_refused() {
    let (fixture, mut definition) = routed().await;
    let routing = definition.routing.as_mut().unwrap();
    routing.consumers[0].profile.model.model = "gpt-other".into();
    assert!(definition.validate().is_err());
    assert_eq!(
        restore(fixture, definition).await,
        Err(ResolveError::InvalidDefinition)
    );
}
