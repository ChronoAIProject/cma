//! The consumer views and key hashes of a resolved channel.
use super::*;
use cmaeg_core::{channel::ChannelDefinition, ids::ChannelId, seal::ChannelPointer};

use crate::channels::routing_fixture;

fn resolved(definition: ChannelDefinition) -> ResolvedChannel {
    let mut consumer_keys = vec![(ConsumerId::default_consumer(), [7; 32])];
    for (index, id) in definition
        .consumers()
        .skip(1)
        .map(|view| view.id.clone())
        .enumerate()
    {
        consumer_keys.push((id, [8 + index as u8; 32]));
    }
    ResolvedChannel {
        id: ChannelId::new("cmaeg1.ch.consumers-fixture").unwrap(),
        pointer: ChannelPointer {
            v: 1,
            record_id: definition.record_id.clone(),
            record_url: "https://cma.example/api/v1/gateway/channel-records/record-1".into(),
        },
        definition,
        agent_key_hash: [7; 32],
        consumer_keys,
        loaded_at: std::time::Instant::now(),
        version: 1,
    }
}

fn channel() -> ResolvedChannel {
    resolved(
        serde_json::from_str(include_str!(
            "../../../cmaeg-core/tests/fixtures/channel.json"
        ))
        .unwrap(),
    )
}

#[test]
fn the_default_consumer_resolves_with_its_restored_key_hash() {
    let channel = channel();
    let default = ConsumerId::default_consumer();
    for id in [None, Some(&default)] {
        let view = channel.consumer(id).unwrap();
        assert!(view.id.is_default());
        assert_eq!(view.provider.binding_id.as_str(), "binding-1");
        assert_eq!(channel.consumer_key_hash(id), Some(&[7; 32]));
    }
}

#[test]
fn an_unknown_consumer_has_no_view_and_no_key_hash() {
    let channel = channel();
    let other = ConsumerId::parse("csm_0123456789ab").unwrap();
    assert!(channel.consumer(Some(&other)).is_none());
    assert!(channel.consumer_key_hash(Some(&other)).is_none());
}

#[test]
fn an_additional_consumer_resolves_with_its_own_key_hash() {
    let channel = resolved(routing_fixture::definition(routing_fixture::routed()));
    for (id, hash) in [
        (routing_fixture::REVIEWER, [8; 32]),
        (routing_fixture::NYX, [9; 32]),
    ] {
        let id = ConsumerId::parse(id).unwrap();
        assert_eq!(channel.consumer(Some(&id)).unwrap().id, &id);
        assert_eq!(channel.consumer_key_hash(Some(&id)), Some(&hash));
    }
    assert_eq!(channel.consumer_key_hash(None), Some(&[7; 32]));
    let other = ConsumerId::parse("csm_0123456789ab").unwrap();
    assert!(channel.consumer_key_hash(Some(&other)).is_none());
}
