use super::*;
use layer2_core::bridge::ingress::{ChannelControlReceipt, InboxReceipt};
use serde_json::{json, Value};

fn fixture(version: u16, kind: &str) -> Value {
    let source = json!({
        "inbox_id":"a".repeat(64),"owner":{"kind":"person","subject":"owner"},
        "pin":{"bridge_id":"brg_fixture","config_revision":1},
        "bot_id":"bot","key_id":"key","message_id":"message",
        "correlation_id":"correlation","issuer":"https://issuer.example",
        "body_sha256":"b".repeat(64)
    });
    let receipt = match kind {
        "message" => json!({"inbox_id":"a".repeat(64),"bridge_id":"brg_fixture",
            "message_id":"message","accepted_at":100,"admission":"accepted",
            "execution_hold":"worker_unavailable"}),
        "control_refused" => json!({"version":1,"callback_id":"a".repeat(64),
            "bridge_id":"brg_fixture","message_id":"message","persisted_at":100,
            "code":"channel_controls_disabled"}),
        "control_accepted" => json!({"version":2,"callback_id":"a".repeat(64),
            "bridge_id":"brg_fixture","message_id":"message","persisted_at":100,
            "intent_id":format!("cci_{}","c".repeat(64))}),
        _ => unreachable!(),
    };
    json!({"version":version,"source":source,"original":source,
        "receipt":{"kind":kind,"receipt":receipt},
        "provider":{"key_sha256":"d".repeat(64),"semantic_sha256":"e".repeat(64)}})
}

#[test]
fn v1_message_and_refusal_bytes_remain_readable_and_unchanged() {
    for kind in ["message", "control_refused"] {
        let original = fixture(1, kind);
        let value: CallbackDisposition = serde_json::from_value(original.clone()).unwrap();
        value.validate().unwrap();
        assert_eq!(serde_json::to_value(&value).unwrap(), original);
        let document = value.document().unwrap();
        assert_eq!(
            CallbackDisposition::read(&document)
                .unwrap()
                .document()
                .unwrap(),
            document
        );
        let old: V1Acceptance = serde_json::from_value(original["receipt"].clone()).unwrap();
        assert_eq!(
            serde_json::to_vec(&old).unwrap(),
            serde_json::to_vec(&value.receipt).unwrap()
        );
        let old_id = match old {
            V1Acceptance::Message(receipt) => receipt.inbox_id,
            V1Acceptance::ControlRefused(receipt) => receipt.callback_id,
        };
        assert_eq!(old_id, value.receipt.callback_id());
    }
}

#[test]
fn accepted_control_is_a_v2_intent_receipt_and_never_a_message() {
    let original = fixture(2, "control_accepted");
    let value: CallbackDisposition = serde_json::from_value(original.clone()).unwrap();
    value.validate().unwrap();
    assert!(value.receipt.clone().into_message().is_none());
    let document = value.document().unwrap();
    assert_eq!(
        CallbackDisposition::read(&document)
            .unwrap()
            .document()
            .unwrap(),
        document
    );
    let provider = value.provider_document().unwrap().unwrap();
    assert_eq!(
        CallbackDisposition::read_provider(&provider, &"d".repeat(64))
            .unwrap()
            .document()
            .unwrap(),
        document
    );
    // Exact old enum serde boundary cannot interpret an accepted intent as a
    // Message or refusal. This alone does not establish rollout compatibility.
    assert!(serde_json::from_value::<V1Acceptance>(original["receipt"].clone()).is_err());
}

#[test]
fn cross_version_receipts_and_changed_intent_identity_fail_closed() {
    for (version, kind) in [
        (1, "control_accepted"),
        (2, "message"),
        (2, "control_refused"),
        (3, "control_accepted"),
    ] {
        let value: CallbackDisposition = serde_json::from_value(fixture(version, kind)).unwrap();
        assert_eq!(value.validate(), Err(CallbackError::Unavailable));
    }
    for (field, change) in [
        ("version", json!(1)),
        ("callback_id", json!("f".repeat(64))),
        ("bridge_id", json!("brg_other")),
        ("message_id", json!("other")),
        ("intent_id", json!("cci_")),
        ("intent_id", json!(format!("cci_{}", "A".repeat(64)))),
        ("intent_id", json!(format!("cmd_{}", "c".repeat(64)))),
    ] {
        let mut data = fixture(2, "control_accepted");
        data["receipt"]["receipt"][field] = change;
        let value: CallbackDisposition = serde_json::from_value(data).unwrap();
        assert_eq!(value.validate(), Err(CallbackError::Unavailable), "{field}");
    }
    let value: CallbackDisposition =
        serde_json::from_value(fixture(2, "control_accepted")).unwrap();
    let mut document = value.document().unwrap();
    document
        .get_document_mut("record")
        .unwrap()
        .get_document_mut("receipt")
        .unwrap()
        .get_document_mut("receipt")
        .unwrap()
        .insert("intent_id", format!("cci_{}", "f".repeat(64)));
    assert!(matches!(
        CallbackDisposition::read(&document),
        Err(CallbackError::Unavailable)
    ));
}

// The serialized enum contract at ee9ac699. Deserialize only: no effect path.
#[derive(Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "receipt",
    rename_all = "snake_case",
    deny_unknown_fields
)]
enum V1Acceptance {
    Message(InboxReceipt),
    ControlRefused(ChannelControlReceipt),
}
