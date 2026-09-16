//! Signed adversarial fixtures, not a claim that upstream emits these events.
use layer2_core::bridge::{partition::PartitionRefusal, BridgePlatform};
use serde_json::{json, Value};

#[derive(Debug, Clone, Copy)]
pub(crate) enum EventCase {
    Edit,
    Delete,
    Unknown,
    Nontext,
    RawNontext(&'static str),
    Bot,
    Loop,
    TextMismatch,
    SenderMismatch,
    ChatMismatch,
    MissingRaw,
    ConflictingDiscriminator,
}

pub(crate) fn platforms() -> [BridgePlatform; 5] {
    [
        BridgePlatform::Telegram,
        BridgePlatform::TelegramNew,
        BridgePlatform::Discord,
        BridgePlatform::Lark,
        BridgePlatform::Feishu,
    ]
}

pub(crate) fn cases(platform: &BridgePlatform) -> Vec<EventCase> {
    use EventCase::*;
    let mut cases = vec![
        Edit,
        Delete,
        Unknown,
        Nontext,
        Bot,
        TextMismatch,
        SenderMismatch,
        ChatMismatch,
        MissingRaw,
        ConflictingDiscriminator,
    ];
    if matches!(
        platform,
        BridgePlatform::Telegram | BridgePlatform::TelegramNew
    ) {
        cases.extend(
            [
                "contact",
                "location",
                "venue",
                "poll",
                "dice",
                "game",
                "invoice",
                "successful_payment",
            ]
            .map(RawNontext),
        );
        cases.push(Loop);
    } else if *platform == BridgePlatform::Discord {
        cases.extend([RawNontext("system"), Loop]);
    } else {
        cases.push(RawNontext("post"));
    }
    cases
}

pub(crate) fn text(body: &mut Value, platform: &BridgePlatform, text: &str) {
    body["content"]["text"] = json!(text);
    match platform {
        BridgePlatform::Telegram | BridgePlatform::TelegramNew => {
            body["raw_platform_data"]["message"]["text"] = json!(text)
        }
        BridgePlatform::Discord => body["raw_platform_data"]["d"]["content"] = json!(text),
        BridgePlatform::Lark | BridgePlatform::Feishu => {
            body["raw_platform_data"]["event"]["message"]["content"] =
                json!(json!({"text":text}).to_string())
        }
        BridgePlatform::Slack | BridgePlatform::Whatsapp | BridgePlatform::X => {
            unreachable!("separate boundary fixtures")
        }
    }
}

impl EventCase {
    /// None means malformed signed input must fail before durable acceptance.
    pub(crate) fn expected(self, platform: &BridgePlatform) -> Option<PartitionRefusal> {
        use EventCase::*;
        use PartitionRefusal as R;
        let telegram = matches!(
            platform,
            BridgePlatform::Telegram | BridgePlatform::TelegramNew
        );
        Some(match self {
            SenderMismatch | ChatMismatch => return None,
            ConflictingDiscriminator if telegram => return None,
            Edit if telegram || *platform == BridgePlatform::Discord => R::MessageEdit,
            Edit | Delete | Unknown | MissingRaw | ConflictingDiscriminator => R::UnknownSender,
            Nontext => R::ContentUnsupported,
            RawNontext(_) if telegram => R::ContentUnsupported,
            RawNontext(_) | TextMismatch => R::UnknownEvent,
            Bot => R::BotSender,
            Loop => R::CallbackLoop,
        })
    }

    pub(crate) fn apply(self, body: &mut Value, platform: &BridgePlatform) {
        use EventCase::*;
        match self {
            Nontext => {
                body["content"]["type"] = json!("unsupported-fixture");
                return;
            }
            TextMismatch => {
                body["content"]["text"] = json!("CMA-CONTROL/1 approve ccr_other");
                return;
            }
            SenderMismatch => {
                body["sender"]["platform_id"] = json!("different-sender");
                return;
            }
            ChatMismatch => {
                body["conversation"]["platform_id"] = json!("different-chat");
                return;
            }
            MissingRaw => {
                body.as_object_mut().unwrap().remove("raw_platform_data");
                return;
            }
            _ => {}
        }
        let raw = &mut body["raw_platform_data"];
        match platform {
            BridgePlatform::Telegram | BridgePlatform::TelegramNew => match self {
                Edit => {
                    let message = raw.as_object_mut().unwrap().remove("message").unwrap();
                    raw["edited_message"] = message;
                }
                Delete => {
                    *raw = json!({"update_id":99,"deleted_business_messages":{
                        "business_connection_id":"synthetic-connection",
                        "chat":{"id":-100123,"type":"supergroup"},"message_ids":[42]}});
                }
                Unknown => *raw = json!({"update_id":99,"unknown_fixture_event":{"text":"hello"}}),
                RawNontext(field) => raw["message"][field] = json!({"fixture":true}),
                Bot => raw["message"]["from"]["is_bot"] = json!(true),
                Loop => raw["message"]["via_bot"] = json!({"id":987,"is_bot":true}),
                ConflictingDiscriminator => {
                    raw["callback_query"] = json!({"data":"fixture-action"})
                }
                _ => unreachable!(),
            },
            BridgePlatform::Discord => match self {
                Edit => raw["t"] = json!("MESSAGE_UPDATE"),
                Delete => raw["t"] = json!("MESSAGE_DELETE"),
                Unknown => raw["t"] = json!("UNKNOWN_FIXTURE_EVENT"),
                RawNontext(_) => raw["d"]["type"] = json!(7),
                Bot => raw["d"]["author"]["bot"] = json!(true),
                Loop => raw["d"]["webhook_id"] = json!("fixture-echo"),
                ConflictingDiscriminator => raw["op"] = json!(1),
                _ => unreachable!(),
            },
            BridgePlatform::Lark | BridgePlatform::Feishu => match self {
                Edit => raw["header"]["event_type"] = json!("fixture.message_edit"),
                Delete => raw["header"]["event_type"] = json!("fixture.message_delete"),
                Unknown => raw["header"]["event_type"] = json!("fixture.unknown_event"),
                RawNontext(kind) => raw["event"]["message"]["message_type"] = json!(kind),
                Bot => raw["event"]["sender"]["sender_type"] = json!("app"),
                ConflictingDiscriminator => raw["schema"] = json!("unknown-schema"),
                _ => unreachable!(),
            },
            BridgePlatform::Slack | BridgePlatform::Whatsapp | BridgePlatform::X => {
                unreachable!("separate boundary fixtures")
            }
        }
    }
}
