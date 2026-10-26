use super::{check, WriteFlags};
use cmaeg_core::{channel::ChannelDefinition, profile::EventWorkerProfile};
use serde_json::{json, Value};

fn raw(platform: &str) -> ChannelDefinition {
    let mut value: Value = serde_json::from_str(include_str!(
        "../../../cmaeg-core/tests/fixtures/channel.json"
    ))
    .unwrap();
    value["sources"] = json!([{"type":"nyxid_relay","issuer":"https://nyx.example",
        "jwks_url":"https://nyx.example/.well-known/jwks.json","key_id":"fixture-signing-key",
        "route_ids":[format!("route-{platform}")],"platform":platform,"verify_raw_event":true,"admission":{"type":"all"}}]);
    value["partition"] = json!({"version":1,"key":"conversation_and_sender"});
    value["secrets"]["channel_key"] = "fixture-relay-credential".into();
    let profile: EventWorkerProfile = serde_json::from_value(value["profile"].clone()).unwrap();
    value["profile_digest"] = profile.digest().into();
    serde_json::from_value(value).unwrap()
}
#[test]
fn raw_flag_does_not_enable_broader_source_or_partition_writes() {
    let flags = WriteFlags {
        relay_raw_verification: true,
        ..Default::default()
    };
    for platform in ["discord", "whatsapp"] {
        let plain = raw(platform);
        plain.validate().unwrap();
        check(None, &plain, flags).unwrap();
        let mut multiple = plain.clone();
        multiple.sources.push(
            raw(if platform == "discord" {
                "whatsapp"
            } else {
                "discord"
            })
            .sources
            .remove(0),
        );
        assert_eq!(
            check(None, &multiple, flags).unwrap_err().message,
            "source variant is duplicated"
        );
        let mut partition = plain.clone();
        partition.partition.custom_key = Some(cmaeg_core::channel::PartitionKey::PerEvent);
        assert_eq!(
            check(None, &partition, flags).unwrap_err().message,
            "dynamic_sources_disabled"
        );
        // Enabling the broad gate alone cannot introduce raw verification.
        let broad = WriteFlags {
            dynamic_sources: true,
            ..Default::default()
        };
        assert_eq!(
            check(None, &plain, broad).unwrap_err().message,
            "relay_raw_verification_writes_disabled"
        );
    }
}
#[test]
fn raw_flag_does_not_change_other_reader_first_gates() {
    let mut next = raw("whatsapp");
    next.routing =
        crate::channels::routing_fixture::definition(crate::channels::routing_fixture::routed())
            .routing;
    let error = check(
        None,
        &next,
        WriteFlags {
            relay_raw_verification: true,
            ..Default::default()
        },
    )
    .unwrap_err();
    assert_eq!(error.message, "multi_consumer_disabled");
}

#[test]
fn changing_only_verification_preserves_an_existing_extended_topology() {
    use cmaeg_core::channel::Source;
    let mut current = raw("discord");
    current.sources.push(raw("whatsapp").sources.remove(0));
    for source in &mut current.sources {
        if let Source::NyxidRelay {
            verify_raw_event,
            trust_normalized,
            ..
        } = source
        {
            *verify_raw_event = false;
            *trust_normalized = true;
        }
    }
    current.validate().unwrap();
    let mut next = current.clone();
    if let Source::NyxidRelay {
        verify_raw_event,
        trust_normalized,
        ..
    } = &mut next.sources[0]
    {
        *verify_raw_event = true;
        *trust_normalized = false;
    }
    next.validate().unwrap();
    let flags = WriteFlags {
        relay_raw_verification: true,
        ..Default::default()
    };
    check(Some(&current), &next, flags).unwrap();
    assert_eq!(
        check(Some(&current), &next, WriteFlags::default())
            .unwrap_err()
            .message,
        "relay_raw_verification_writes_disabled"
    );
    if let Source::NyxidRelay { route_ids, .. } = &mut next.sources[0] {
        route_ids.push("another-route".into());
    }
    assert_eq!(
        check(Some(&current), &next, flags).unwrap_err().message,
        "source variant is duplicated"
    );
}
