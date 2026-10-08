//! The service's own checks of a routed definition, which need the
//! authenticated owner and the key hashing the core does not know.
use super::{validate_consumer_keys, validate_definition, WriteFlags};
use cmaeg_core::{
    channel::{ChannelDefinition, Secret},
    ValidationError,
};

use crate::channels::routing_fixture;
use routing_fixture::{agent_key, definition, digest_of, routed, NYX, REVIEWER};

/// Validate as a writer behind the multi-consumer flag would; with the flag
/// off the write gate refuses every routed write before these checks run.
fn check(definition: &ChannelDefinition) -> Result<(), ValidationError> {
    let flags = WriteFlags {
        multi_consumer: true,
        ..WriteFlags::default()
    };
    validate_definition(None, definition, flags)
}

#[test]
fn a_routed_definition_owned_and_keyed_consistently_is_valid() {
    let routed = definition(routed());
    check(&routed).unwrap();
    validate_consumer_keys(&routed).unwrap();
}

#[test]
fn every_consumer_acts_for_the_channel_owner() {
    let mut value = routed();
    let profile = &mut value["routing"]["consumers"][1]["profile"];
    profile["metadata"]["owner"]["subject"] = "someone-else".into();
    let digest = digest_of(profile);
    value["routing"]["consumers"][1]["profile_digest"] = digest.into();
    assert_eq!(
        check(&definition(value)),
        Err(ValidationError::new(
            "/routing/consumers/1/profile/metadata/owner/subject",
            "must match the authenticated channel owner",
        ))
    );
}

#[test]
fn a_consumer_key_must_hash_to_its_public_hash() {
    let mut routed = definition(routed());
    let reviewer = &mut routed.secrets.consumer_keys[0];
    assert_eq!(reviewer.0.as_str(), REVIEWER);
    reviewer.1 = Secret::new("nyx_fixture_another_key");
    let mismatch = Err(ValidationError::new(
        "/routing/consumers/0/agent_key_hash",
        "does not match the consumer's agent key",
    ));
    assert_eq!(validate_consumer_keys(&routed), mismatch);
    assert_eq!(check(&routed), mismatch);
    // A consumer without a key is refused by the reader before this check.
    routed.secrets.consumer_keys[0].1 = Secret::new(agent_key(REVIEWER));
    routed
        .secrets
        .consumer_keys
        .retain(|(id, _)| id.as_str() != NYX);
    assert_eq!(
        validate_consumer_keys(&routed),
        Err(ValidationError::new(
            "/routing/consumers/1/agent_key_hash",
            "does not match the consumer's agent key",
        ))
    );
}

#[test]
fn a_legacy_definition_has_no_consumer_keys_to_check() {
    let mut value = routed();
    value.as_object_mut().unwrap().remove("routing");
    value["secrets"]
        .as_object_mut()
        .unwrap()
        .remove("consumer_keys");
    validate_consumer_keys(&definition(value)).unwrap();
}
