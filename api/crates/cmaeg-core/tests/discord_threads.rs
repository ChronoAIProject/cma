use cmaeg_core::seal::{EventRefPayload, ReplyAddress};
use serde_json::json;

#[test]
fn retained_native_reply_has_identical_wire_shape_without_thread_evidence() {
    let legacy = json!({"type":"discord","bot_id":"77","channel_id":"99","message_id":"700"});
    let address: ReplyAddress = serde_json::from_value(legacy.clone()).unwrap();
    assert!(matches!(
        &address,
        ReplyAddress::Discord {
            thread_id: None,
            ..
        }
    ));
    assert_eq!(serde_json::to_value(&address).unwrap(), legacy);
    let mut with_thread = legacy;
    with_thread["thread_id"] = "99".into();
    let address: ReplyAddress = serde_json::from_value(with_thread.clone()).unwrap();
    assert_eq!(serde_json::to_value(address).unwrap(), with_thread);
}

#[test]
fn sealed_thread_authority_cannot_retarget_its_original_native_channel() {
    let mut event: EventRefPayload = serde_json::from_value(json!({
        "v":1,"channel_id":"cmaeg1.ch.fixture","record_id":"record-1",
        "event_id":"discord:77:700","partition_key":"discord:99:99",
        "conversation_id":"conversation-1","expires_at":100,"consumer":null,"lineage":null,
        "reply":{"type":"discord","bot_id":"77","channel_id":"99","message_id":"700","thread_id":"99"}
    })).unwrap();
    assert!(event.validate().is_ok());
    let ReplyAddress::Discord { thread_id, .. } = &mut event.reply else {
        panic!()
    };
    *thread_id = Some("100".into());
    assert_eq!(event.validate().unwrap_err().pointer, "/reply/thread_id");
}
