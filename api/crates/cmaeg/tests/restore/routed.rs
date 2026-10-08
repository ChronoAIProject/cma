//! Restoring a routed channel: its additional consumers are checked as the
//! default one is, and a stored definition that fails is never served.
use super::{routing_fixture, support::Fixture};
use cmaeg::{channels::resolver::ResolveError, ChannelCacheConfig};
use cmaeg_core::channel::{ChannelDefinition, Secret};

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
async fn a_consistent_routed_channel_restores() {
    let (fixture, definition) = routed().await;
    restore(fixture, definition).await.unwrap();
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
