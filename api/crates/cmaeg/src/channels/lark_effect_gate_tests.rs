use super::write_gate::{check, WriteFlags};
use cmaeg_core::channel::ChannelDefinition;
use serde_json::json;

fn definition() -> ChannelDefinition {
    let mut raw = super::definition::fixture::with_agent_key("lark-agent").to_blob_json();
    raw["sources"] = json!([{"type":"nyxid_relay","issuer":"https://nyx.example","jwks_url":"https://nyx.example/jwks","key_id":"key","route_ids":["route-one"],"platform":"lark","bot_id":"ou_source","admission":{"type":"all"},"lark_message_effects":{"connection":{"user_service_id":"connection-one","catalog_service_id":"catalog-one","app_id":"cli_source"},"delete":true,"reaction":false}}]);
    raw["secrets"]["channel_key"] = json!("lark-channel-key");
    raw["partition"]["key"] = json!("conversation_and_sender");
    serde_json::from_value(raw).unwrap()
}
#[test]
fn lark_effects_require_their_own_gate_and_allow_exact_carry_or_removal() {
    let opted = definition();
    opted.validate().unwrap();
    assert!(check(
        None,
        &opted,
        WriteFlags {
            lark_resources: true,
            ..Default::default()
        }
    )
    .is_err());
    check(
        None,
        &opted,
        WriteFlags {
            lark_message_effects: true,
            ..Default::default()
        },
    )
    .unwrap();
    let mut changed = opted.clone();
    changed.name = "renamed".into();
    check(Some(&opted), &changed, WriteFlags::default()).unwrap();
    if let cmaeg_core::channel::Source::NyxidRelay {
        lark_message_effects: Some(binding),
        ..
    } = &mut changed.sources[0]
    {
        binding.reaction = true;
    }
    assert!(check(Some(&opted), &changed, WriteFlags::default()).is_err());
    if let cmaeg_core::channel::Source::NyxidRelay {
        lark_message_effects,
        ..
    } = &mut changed.sources[0]
    {
        *lark_message_effects = None;
    }
    check(Some(&opted), &changed, WriteFlags::default()).unwrap();
}
#[test]
fn lark_effect_binding_refuses_unverified_or_unpinned_source_and_empty_operations() {
    for (pointer, value) in [
        ("/sources/0/platform", json!("telegram")),
        ("/sources/0/trust_normalized", json!(true)),
        ("/sources/0/bot_id", json!("cli_wrong")),
        ("/sources/0/lark_message_effects/delete", json!(false)),
        (
            "/sources/0/lark_message_effects/connection/app_id",
            json!("ou_wrong"),
        ),
        ("/secrets/channel_key", serde_json::Value::Null),
    ] {
        let mut raw = definition().to_blob_json();
        if pointer.ends_with("trust_normalized") {
            raw["sources"][0]["trust_normalized"] = value;
        } else {
            *raw.pointer_mut(pointer).unwrap() = value;
        }
        let definition: ChannelDefinition = serde_json::from_value(raw).unwrap();
        assert!(definition.validate().is_err(), "{pointer}");
    }
}
