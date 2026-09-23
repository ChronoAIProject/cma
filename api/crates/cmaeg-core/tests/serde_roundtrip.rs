use cmaeg_core::{
    activity::Activity,
    channel::{ChannelDefinition, ChannelSecrets},
    cloudevent::CloudEvent,
    message::OutboundMessage,
    profile::{canonical_json, EventWorkerProfile},
};

fn fixture(name: &str) -> serde_json::Value {
    let path = format!("tests/fixtures/{name}.json");
    serde_json::from_str(&std::fs::read_to_string(path).expect("fixture")).expect("JSON fixture")
}

#[test]
fn canonical_json_sorts_objects_recursively_and_preserves_arrays() {
    let value = serde_json::json!({"b": 1, "a": [{"d": 2, "c": 3}]});
    assert_eq!(canonical_json(value), r#"{"a":[{"c":3,"d":2}],"b":1}"#);
}

#[test]
fn profile_digest_is_stable() {
    let profile: EventWorkerProfile =
        serde_json::from_value(fixture("channel")["profile"].clone()).expect("profile fixture");
    assert_eq!(profile.digest(), profile.digest());
    assert!(profile.digest().starts_with("sha256:"));
}

#[test]
fn activity_fixture_round_trips() {
    let value = fixture("activity");
    let activity: Activity = serde_json::from_value(value.clone()).expect("activity");
    assert_eq!(
        serde_json::to_value(activity).expect("activity JSON"),
        value
    );
}

#[test]
fn outbound_message_fixture_round_trips() {
    let value = fixture("outbound_message");
    let message: OutboundMessage = serde_json::from_value(value.clone()).expect("message");
    message.validate().expect("valid message");
    assert_eq!(serde_json::to_value(message).expect("message JSON"), value);
}

#[test]
fn cloud_event_fixture_round_trips() {
    let value = fixture("cloudevent");
    let event: CloudEvent = serde_json::from_value(value.clone()).expect("CloudEvent");
    event.validate().expect("valid CloudEvent");
    assert_eq!(serde_json::to_value(event).expect("event JSON"), value);
}

#[test]
fn channel_public_view_round_trips_without_private_secrets() {
    let value = fixture("channel");
    let mut channel: ChannelDefinition = serde_json::from_value(value).expect("channel");
    channel.secrets.channel_key = Some(cmaeg_core::channel::Secret::new("super-secret"));
    let public = channel.to_public_json();
    assert_eq!(
        public["secrets"]["agent_key_hash"],
        channel.secrets.agent_key_hash
    );
    assert!(!public.to_string().contains("super-secret"));
    let decoded: ChannelDefinition = serde_json::from_value(public).expect("public channel");
    assert_eq!(decoded.record_id, channel.record_id);
    assert_eq!(
        decoded.secrets.agent_key_hash,
        channel.secrets.agent_key_hash
    );
    assert_eq!(
        decoded.secrets.channel_key,
        ChannelSecrets::default().channel_key
    );
}
