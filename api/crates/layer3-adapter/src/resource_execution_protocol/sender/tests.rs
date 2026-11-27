use super::*;
use crate::resource_execution_protocol::{decode_message, MemberKey};
use serde_json::{json, Value};

fn receipt(shared: bool) -> SenderStopReceiptV1 {
    let digest = "a".repeat(64);
    let channel = json!({
        "issuer": digest, "record_incarnation": digest,
        "owner_kind": "person", "owner_subject": digest, "generation": 2,
    });
    let family = json!({"origin": digest, "share_id": "share", "generation": 3});
    let (closing, basis) = if shared {
        let revocation = json!({"operation_id": digest, "family": family, "watermark": 4});
        let grant = json!({
            "family": family, "share_revision": 2, "policy_revision": 3,
            "target_ancestry": digest, "capability": "operate_agent", "actor": digest,
        });
        (
            json!({"kind": "share", "revocation": revocation}),
            json!({"kind": "share", "grant": grant}),
        )
    } else {
        let closing = json!({
            "kind": "channel", "operation_id": digest, "authority": channel, "watermark": 4,
        });
        (closing, json!({"kind": "owner"}))
    };
    serde_json::from_value(json!({
        "protocol": 1,
        "request": {
            "protocol": 1, "registration": digest,
            "sender": {"instance": "gateway", "incarnation": digest, "boot": "boot"},
            "closing": closing, "channel": channel, "basis": basis,
            "source_consumer": digest, "custody": digest, "origin": digest,
            "credential_config": digest, "target": digest,
            "effect_id": "effect", "request_digest": digest,
        },
        "durable_watermark": 4, "barrier_sequence": 5, "stop_sequence": 6,
        "submitters_closed": true,
        "effects": {"high_water": 1, "digest": digest, "unresolved_count": 1},
    }))
    .unwrap()
}

fn check(actual: &SenderStopReceiptV1, expected: &SenderStopReceiptV1) -> ProtocolResult<()> {
    actual.validate_for(
        &expected.request,
        &expected.request.sender,
        expected.barrier_sequence,
        &expected.effects,
    )
}

#[test]
fn old_members_decode_and_sender_receipts_round_trip_with_unresolved_effects() {
    let release = br#"{"kind":"release","release":{"response_id":"reply","content_fingerprint":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","receiver":{"kind":"release_producer","instance":"producer","incarnation":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","boot":"boot"}}}"#;
    let execution = br#"{"kind":"execution","execution":{"command_id":"cmd","command_fingerprint":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","dispatch_sequence":1,"prompt_id":"prompt","turn_id":"turn","agent_id":"agent","receiver":{"kind":"runtime","instance":"runtime","incarnation":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","boot":"boot"},"attempt":1}}"#;
    for bytes in [release.as_slice(), execution.as_slice()] {
        let member: MemberKey = decode_message(bytes).unwrap();
        member.validate().unwrap();
    }
    for shared in [false, true] {
        let original = receipt(shared);
        let encoded = serde_json::to_vec(&original).unwrap();
        let decoded: SenderStopReceiptV1 = decode_message(&encoded).unwrap();
        check(&decoded, &original).unwrap();
        assert!(decoded.is_exact_replay_of(&original));
    }
}

#[test]
fn altered_stop_evidence_is_refused() {
    let original = receipt(true);
    let other_digest = json!("b".repeat(64));
    for path in [
        "/request/registration",
        "/request/closing/revocation/family/origin",
        "/request/sender/incarnation",
        "/request/channel/owner_subject",
        "/request/channel/issuer",
        "/request/channel/record_incarnation",
        "/request/basis/grant/actor",
        "/request/basis/grant/target_ancestry",
        "/request/target",
        "/request/origin",
        "/request/custody",
        "/request/source_consumer",
        "/request/credential_config",
        "/request/request_digest",
        "/effects/digest",
    ] {
        rejects_mutation(&original, path, other_digest.clone());
    }
    for (path, value) in [
        ("/protocol", json!(2)),
        ("/request/protocol", json!(2)),
        ("/request/sender/instance", json!("other")),
        ("/request/sender/boot", json!("other")),
        ("/request/effect_id", json!("changed")),
        ("/request/closing/revocation/watermark", json!(9)),
        ("/request/closing/revocation/family/generation", json!(9)),
        ("/request/channel/generation", json!(9)),
        ("/request/basis", json!({"kind": "owner"})),
        ("/request/closing", json!(receipt(false).request.closing)),
        ("/durable_watermark", json!(9)),
        ("/barrier_sequence", json!(4)),
        ("/stop_sequence", json!(5)),
        ("/stop_sequence", json!(4)),
        ("/submitters_closed", json!(false)),
        ("/effects/unresolved_count", json!(2)),
        ("/effects/high_water", json!(u64::MAX)),
    ] {
        rejects_mutation(&original, path, value);
    }
    let mut different = original.clone();
    different.effects.unresolved_count = 0;
    assert!(!different.is_exact_replay_of(&original));
    let mut replacement = original.request.sender.clone();
    replacement.boot = OpaqueId::new("replacement").unwrap();
    assert_eq!(
        original.validate_for(
            &original.request,
            &replacement,
            original.barrier_sequence,
            &original.effects
        ),
        Err(ProtocolError::AudienceMismatch)
    );
}

fn rejects_mutation(original: &SenderStopReceiptV1, path: &str, value: Value) {
    let mut message = json!(original);
    *message.pointer_mut(path).unwrap() = value;
    let changed = decode_message(&serde_json::to_vec(&message).unwrap()).unwrap();
    assert!(check(&changed, original).is_err(), "{path}");
}

#[test]
fn decoder_rejects_an_unknown_field() {
    let mut message = json!(receipt(false));
    message["request"]["basis"]["grant"] = json!({});
    assert!(decode_message::<SenderStopReceiptV1>(&serde_json::to_vec(&message).unwrap()).is_err());
}
