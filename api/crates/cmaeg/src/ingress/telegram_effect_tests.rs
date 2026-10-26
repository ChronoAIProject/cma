use super::*;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use cmaeg_core::{channel::ChannelDefinition, seal::ChannelPointer};

#[test]
fn exact_native_reply_proof_never_substitutes_a_foreign_bot() {
    for (parent, expected) in [(77, Some(true)), (88, Some(false))] {
        assert_eq!(
            exact_reply_to_bot(
                Some(&json!({"message":{"reply_to_message":{"from":{"id":parent,"is_bot":true}}}})),
                "77"
            ),
            expected
        );
        assert_eq!(
            exact_reply_to_bot(
                Some(&json!({"callback_query":{"message":{"from":{"id":parent,"is_bot":true}}}})),
                "77"
            ),
            expected
        );
    }
    assert_eq!(
        exact_reply_to_bot(Some(&json!({"message":{}})), "77"),
        Some(false)
    );
    assert_eq!(
        exact_reply_to_bot(Some(&json!({"message":{"reply_to_message":{}}})), "77"),
        None
    );
}

#[test]
fn enabled_native_ingress_persists_reply_evidence_but_disabled_ingress_keeps_old_wire() {
    let keys = MasterKeyRing::parse(&format!("1:{}", STANDARD.encode([5; 32]))).unwrap();
    let definition: ChannelDefinition = serde_json::from_str(include_str!(
        "../../../cmaeg-core/tests/fixtures/channel.json"
    ))
    .unwrap();
    let pointer = ChannelPointer {
        v: 1,
        record_id: definition.record_id.clone(),
        record_url: "https://cma.example/api/v1/gateway/channel-records/fixture".into(),
    };
    let channel = Arc::new(ResolvedChannel {
        id: pointer.seal_id(&keys).unwrap(),
        pointer,
        definition,
        agent_key_hash: [0; 32],
        consumer_keys: vec![],
        loaded_at: Instant::now(),
        version: 1,
    });
    for enabled in [false, true] {
        let raw = json!({"update_id":1,"message":{"message_id":2,"date":1800000000,"chat":{"id":-100,"type":"supergroup"},"from":{"id":42,"is_bot":false},"text":"hello","reply_to_message":{"message_id":9,"from":{"id":77,"is_bot":true}}}});
        let Outcome::Admitted(observed) = observe(raw, "77", None, false, false).unwrap() else {
            panic!("native event");
        };
        let Built::Launch(request) = launch(
            channel.clone(),
            "77",
            *observed,
            &keys,
            Instant::now(),
            1800000000,
            enabled,
        )
        .unwrap() else {
            panic!("launch");
        };
        let event = request.event_ref.open(&keys, 1800000001).unwrap().payload;
        let wire = serde_json::to_value(&event).unwrap();
        assert_eq!(wire.get("telegram_admission").is_some(), enabled);
        assert!(event.verifies_activity(&request.activity));
        if enabled {
            let proof = event.telegram_admission.unwrap();
            assert_eq!(proof.actor_id, "42");
            assert_eq!(proof.replied_to_bot, Some(true));
            assert_eq!(proof.mentioned, Some(false));
        }
    }
}
