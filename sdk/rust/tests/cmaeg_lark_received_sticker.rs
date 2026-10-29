use cma_sdk::cmaeg::types::OutboundMessage;
use serde_json::json;

#[test]
fn received_sticker_is_optional_and_accepts_no_raw_key_surface() {
    let legacy = json!({"text":"unchanged","blocks":[]});
    let old: OutboundMessage = serde_json::from_value(legacy.clone()).unwrap();
    assert_eq!(serde_json::to_value(old).unwrap(), legacy);
    for enabled in [false, true] {
        let wire = json!({"reply_with_received_sticker":enabled,"blocks":[]});
        let message: OutboundMessage = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(serde_json::to_value(message).unwrap(), wire);
    }
    for value in [
        json!(null),
        json!("true"),
        json!(1),
        json!({"file_key":"arbitrary"}),
    ] {
        assert!(
            serde_json::from_value::<OutboundMessage>(json!({"reply_with_received_sticker":value}))
                .is_err()
        );
    }
}
