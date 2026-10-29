use cma_sdk::cmaeg::types::{OutboundMessage, StructuredContent};
use serde_json::json;

#[test]
fn static_content_round_trips_without_changing_legacy_messages() {
    for content in [
        json!({"type":"contact","phone_number":"+6512345678","first_name":"Alice","last_name":"Example","vcard":"BEGIN:VCARD\r\nEND:VCARD"}),
        json!({"type":"location","latitude":-1.3,"longitude":103.8,"horizontal_accuracy":2.5}),
        json!({"type":"venue","latitude":1.3,"longitude":103.8,"title":"Library","address":"Main Street"}),
    ] {
        let wire = json!({"structured_content":content});
        let message: OutboundMessage = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(serde_json::to_value(message).unwrap(), wire);
    }
    let legacy = json!({"text":"unchanged"});
    let message: OutboundMessage = serde_json::from_value(legacy.clone()).unwrap();
    assert_eq!(serde_json::to_value(message).unwrap(), legacy);
    assert!(serde_json::from_value::<OutboundMessage>(json!({"structured_content":null})).is_err());
    assert!(
        serde_json::from_value::<StructuredContent>(
            json!({"type":"location","latitude":1.0,"longitude":2.0,"live_period":60})
        )
        .is_err()
    );
}
