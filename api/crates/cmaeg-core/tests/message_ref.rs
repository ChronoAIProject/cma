use base64::{engine::general_purpose::STANDARD, Engine as _};
use cmaeg_core::{
    ids::MessageRef,
    seal::{open, seal, ChannelPointer, MasterKeyRing, MessageRefPayload, OpenError, TokenKind},
};
fn keys(kid: u8) -> MasterKeyRing {
    MasterKeyRing::parse(&format!("{kid}:{}", STANDARD.encode([kid; 32]))).unwrap()
}
fn payload(keys: &MasterKeyRing) -> MessageRefPayload {
    let channel_id = ChannelPointer {
        v: 1,
        record_id: "record-1".parse().unwrap(),
        record_url: "https://cma.example/api/v1/gateway/channel-records/record-1".into(),
    }
    .seal_id(keys)
    .unwrap();
    MessageRefPayload {
        channel_id,
        event_id: "event-1".parse().unwrap(),
        platform_message_id: "platform-message-1".into(),
        route_id: "route-1".into(),
        interaction_record_id: None,
        consumer: None,
    }
}
#[test]
fn stateless_message_refs_round_trip_and_remain_separate_from_event_refs() {
    let keys = keys(1);
    let payload = payload(&keys);
    let reference = MessageRef::seal(&payload, &keys).unwrap();
    assert!(reference.as_str().starts_with("cmaeg1.mr.1."));
    assert_eq!(reference.open(&keys).unwrap().payload, payload);
    assert_eq!(
        serde_json::from_str::<MessageRef>(&serde_json::to_string(&reference).unwrap()).unwrap(),
        reference
    );
    assert!(matches!(
        open::<serde_json::Value>(&keys, TokenKind::EventRef, reference.as_str()),
        Err(OpenError::WrongKind)
    ));
    let reference = MessageRef::new(seal(&keys, TokenKind::EventRef, &payload).unwrap()).unwrap();
    assert!(matches!(reference.open(&keys), Err(OpenError::WrongKind)));
}
#[test]
fn message_refs_enforce_tamper_rotation_payload_and_length_bounds() {
    let old = keys(1);
    let mut payload = payload(&old);
    let reference = MessageRef::seal(&payload, &old).unwrap();
    let both = MasterKeyRing::parse(&format!(
        "2:{},1:{}",
        STANDARD.encode([2; 32]),
        STANDARD.encode([1; 32])
    ))
    .unwrap();
    assert!(reference.open(&both).unwrap().needs_rekey);
    assert!(matches!(
        reference.open(&keys(2)),
        Err(OpenError::UnknownKid(1))
    ));
    let mut corrupted = reference.as_str().to_owned();
    let index = corrupted.len() - 8;
    let replacement = if &corrupted[index..index + 1] == "A" {
        "B"
    } else {
        "A"
    };
    corrupted.replace_range(index..index + 1, replacement);
    assert!(matches!(
        MessageRef::new(corrupted).unwrap().open(&old),
        Err(OpenError::Tampered)
    ));
    payload.platform_message_id.clear();
    assert!(MessageRef::seal(&payload, &old)
        .unwrap()
        .open(&old)
        .is_err());
    assert!(MessageRef::new("plain".repeat(103)).is_err());
    assert!(MessageRef::new(format!("cmaeg1.mr.1.{}", "x".repeat(4096))).is_err());
    assert_eq!(format!("{reference:?}"), "[redacted message ref]");
}
