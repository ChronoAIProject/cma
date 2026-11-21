use super::*;
use serde_json::json;

fn message(kind: &str) -> OutboundMessage {
    serde_json::from_value(
        json!({"native_attachments":[{"kind":kind,"source":{"type":"base64","data":"AQ=="}}]}),
    )
    .unwrap()
}
#[test]
fn issuance_is_destination_specific_and_legacy_whatsapp_stickers_are_unchanged() {
    let telegram = ReplyAddress::Telegram {
        bot_id: "77".into(),
        chat_id: "42".into(),
        thread_id: None,
        message_id: "700".into(),
    };
    let whatsapp = ReplyAddress::Whatsapp {
        app_id: "12345".into(),
        waba_id: "23456".into(),
        phone_number_id: "34567".into(),
        recipient_id: "447700900123".into(),
        message_id: "wamid.original".into(),
    };
    for kind in ["voice", "animation", "sticker"] {
        let message = message(kind);
        assert!(!allowed(false, &telegram, &message));
        assert!(allowed(true, &telegram, &message));
        assert!(!allowed(true, &ReplyAddress::None, &message));
        assert_eq!(allowed(true, &whatsapp, &message), kind != "animation");
        assert_eq!(allowed(false, &whatsapp, &message), kind == "sticker");
    }
    for kind in ["image", "file", "audio", "video"] {
        assert!(allowed(false, &telegram, &message(kind)));
        assert!(allowed(false, &whatsapp, &message(kind)));
    }
}

#[test]
fn native_presentations_do_not_acquire_control_or_album_authority() {
    let telegram = ReplyAddress::Telegram {
        bot_id: "77".into(),
        chat_id: "42".into(),
        thread_id: None,
        message_id: "700".into(),
    };
    for kind in ["voice", "animation", "sticker"] {
        let mut request = message(kind);
        request.blocks = serde_json::from_value(
            json!([{"type":"buttons","id":"choose","options":[{"id":"ok","label":"OK"}]}]),
        )
        .unwrap();
        assert!(!allowed(true, &telegram, &request));
        assert!(!crate::telegram::album_supported(
            &[request.native_attachments[0].kind; 2]
        ));
    }
}
