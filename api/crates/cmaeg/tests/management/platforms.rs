use super::*;
use cmaeg::channels::definition::{build_definition, ChannelCreateRequest};
use cmaeg_core::{
    channel::{ConformanceLevel, Platform, Source},
    ids::{BindingId, RecordId},
};

fn build(body: serde_json::Value) -> Result<Source, cmaeg_core::ValidationError> {
    let request: ChannelCreateRequest = serde_json::from_value(body).unwrap();
    build_definition(
        &request,
        "alice",
        RecordId::new("record").unwrap(),
        BindingId::new("binding").unwrap(),
        ConformanceLevel::L1,
    )
    .map(|definition| definition.sources[0].clone())
}

#[test]
fn legacy_relay_requests_serialize_without_new_fields_so_fingerprints_are_stable() {
    let request: ChannelCreateRequest = serde_json::from_value(create_body()).unwrap();
    let source = serde_json::to_value(&request.sources[0]).unwrap();
    assert!(source.get("bot_id").is_none(), "{source}");
    assert!(source.get("trust_normalized").is_none(), "{source}");
}

#[test]
fn relay_platform_identity_and_trust_are_carried_into_the_definition() {
    let mut body = create_body();
    body["sources"][0]["platform"] = "lark".into();
    body["sources"][0]["bot_id"] = "ou_bot".into();
    let Source::NyxidRelay {
        platform,
        bot_id,
        trust_normalized,
        ..
    } = build(body.clone()).unwrap()
    else {
        panic!("expected a relay source");
    };
    assert_eq!(
        (platform, bot_id.as_deref(), trust_normalized),
        (Platform::Lark, Some("ou_bot"), false)
    );
    body["sources"][0]["bot_id"] = "cli_app".into();
    assert_eq!(
        build(body.clone()).unwrap_err().pointer,
        "/sources/0/bot_id"
    );
    body["sources"][0]["bot_id"] = "ou_bot".into();
    body["sources"][0]["trust_normalized"] = true.into();
    assert_eq!(
        build(body.clone()).unwrap_err().pointer,
        "/sources/0/trust_normalized"
    );
    body["sources"][0]["platform"] = "whatsapp".into();
    body["sources"][0]["bot_id"] = "1234567890".into();
    let Source::NyxidRelay {
        platform,
        trust_normalized,
        ..
    } = build(body).unwrap()
    else {
        panic!("expected a relay source");
    };
    assert_eq!((platform, trust_normalized), (Platform::Whatsapp, true));
}
