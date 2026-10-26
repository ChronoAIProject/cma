//! Every refusal today's source and partition rules can reach, as the exact
//! pointer and message a caller receives.

use serde_json::{json, Value};

use super::{
    allowlist, attachments, channel, custom, custom_source, edit, hash, hashes, lark_relay, listed,
    open, partition, relay, relay_channel, relay_on, relay_source, scoped, sources, telegram,
    telegram_channel, telegram_secret, telegram_source,
};

const COUNT: &str = "must contain one to four sources";
const DUPLICATED: &str = "source variant is duplicated";
const AT_LEAST_ONE: &str = "must contain at least 1 bytes";
const AT_MOST_512: &str = "must contain at most 512 bytes";
const BOUNDED: &str = "must contain a bounded non-empty list";
const UNIQUE: &str = "values must be unique";
const TELEGRAM_ID: &str = "must be a positive Telegram bot ID";
const TELEGRAM_USERNAME: &str = "must contain 5..=32 Telegram username characters without @";
const TOKEN_REQUIRED: &str = "a Telegram token is required";
const TOKEN_IDENTITY: &str = "must match the Telegram bot identity";
const WEBHOOK_SECRET: &str = "must contain 1..=256 Telegram secret characters";
const OPEN_ID: &str =
    "must be the bot's open_id (ou_ followed by at most 125 identifier characters)";
const CUSTOM_KEY: &str = "custom sources require fixed, event_subject, or per_event";
const NEEDS_CUSTOM: &str = "requires a custom source";

const TOKEN: &str = "/secrets/telegram_bot_token";
const WEBHOOK: &str = "/secrets/telegram_webhook_secret";
const JWKS: &str = "/sources/0/jwks_url";
const ROUTES: &str = "/sources/0/route_ids";
const USERNAME: &str = "/sources/0/bot_username";
const BOT_ID: &str = "/sources/0/bot_id";
const ADMISSION: &str = "/sources/0/admission";
const HASHES: &str = "/sources/0/auth/key_hashes";
const POINTER: &str = "/sources/0/attachments/pointer";
const KEY: &str = "/partition/key";

/// Every refusal the source and partition rules can reach, in code order,
/// followed by the order in which they compete with the rest of validation.
pub(super) fn refusals() -> Vec<(Value, &'static str, &'static str)> {
    let long = json!("x".repeat(513));
    let routes: Vec<_> = (0..65).map(|index| format!("route-{index}")).collect();
    let origins: Vec<_> = (0..17).map(|i| format!("https://o{i}.example")).collect();
    let schema = edit(custom(), |source| source["schema"] = json!({}));
    let too_many_sinks = json!(vec![json!({"type":"sync"}); 4]);
    vec![
        // The source list.
        (sources(channel(), json!([])), "/sources", COUNT),
        (
            sources(channel(), json!(vec![relay(); 5])),
            "/sources",
            COUNT,
        ),
        (
            sources(telegram_channel(), json!([telegram(), telegram()])),
            "/sources/1",
            DUPLICATED,
        ),
        (
            sources(relay_channel(), json!([relay(), lark_relay()])),
            "/sources/1",
            DUPLICATED,
        ),
        (
            sources(relay_channel(), json!([relay(), relay()])),
            "/sources/1",
            DUPLICATED,
        ),
        (
            sources(channel(), json!([custom(), custom()])),
            "/sources/1",
            DUPLICATED,
        ),
        (
            sources(
                telegram_channel(),
                json!([telegram(), relay(), custom(), lark_relay()]),
            ),
            "/sources/3",
            DUPLICATED,
        ),
        // Native Telegram.
        (telegram_source("bot_id", json!("0")), BOT_ID, TELEGRAM_ID),
        (telegram_source("bot_id", json!("077")), BOT_ID, TELEGRAM_ID),
        (
            telegram_source("bot_username", json!("bot")),
            USERNAME,
            TELEGRAM_USERNAME,
        ),
        (
            telegram_source("admission", allowlist(json!([]))),
            ADMISSION,
            BOUNDED,
        ),
        (
            telegram_secret("telegram_bot_token", Value::Null),
            TOKEN,
            TOKEN_REQUIRED,
        ),
        (
            telegram_secret("telegram_bot_token", json!("fixture")),
            TOKEN,
            TOKEN_REQUIRED,
        ),
        (
            telegram_secret("telegram_bot_token", json!("78:fixture")),
            TOKEN,
            TOKEN_IDENTITY,
        ),
        (
            telegram_secret("telegram_bot_token", json!("77:")),
            TOKEN,
            TOKEN_IDENTITY,
        ),
        (
            telegram_secret("telegram_webhook_secret", Value::Null),
            WEBHOOK,
            WEBHOOK_SECRET,
        ),
        (
            telegram_secret("telegram_webhook_secret", json!("a b")),
            WEBHOOK,
            WEBHOOK_SECRET,
        ),
        // NyxID relay.
        (
            relay_source("issuer", json!("")),
            "/sources/0/issuer",
            AT_LEAST_ONE,
        ),
        (
            relay_source("issuer", long.clone()),
            "/sources/0/issuer",
            AT_MOST_512,
        ),
        (
            relay_source("key_id", json!("")),
            "/sources/0/key_id",
            AT_LEAST_ONE,
        ),
        (
            relay_source("key_id", long.clone()),
            "/sources/0/key_id",
            AT_MOST_512,
        ),
        (
            relay_source("jwks_url", json!("/relative")),
            JWKS,
            "must be an absolute URL",
        ),
        (
            relay_source("jwks_url", json!("https://example.test/a b")),
            JWKS,
            "must include an absolute host",
        ),
        (
            relay_source("jwks_url", json!("http://example.test/jwks")),
            JWKS,
            "must use https (or local http)",
        ),
        (relay_source("route_ids", json!(routes)), ROUTES, BOUNDED),
        (
            relay_source("route_ids", json!(["same", "same"])),
            ROUTES,
            UNIQUE,
        ),
        (
            relay_source("route_ids", json!([""])),
            "/sources/0/route_ids/0",
            AT_LEAST_ONE,
        ),
        (
            relay_source("route_ids", json!([long])),
            "/sources/0/route_ids/0",
            AT_MOST_512,
        ),
        (
            relay_on("telegram", json!({"bot_username":"@support_bot"})),
            USERNAME,
            TELEGRAM_USERNAME,
        ),
        (
            relay_on("lark", json!({"bot_username":"@support"})),
            USERNAME,
            "must contain 1..=64 letters, digits, '_', '-' or '.' without @",
        ),
        (
            relay_on("telegram", json!({"bot_id":"-1"})),
            BOT_ID,
            "must be the bot's numeric Telegram user ID",
        ),
        (
            relay_on("lark", json!({"bot_id":"cli_a1b2"})),
            BOT_ID,
            OPEN_ID,
        ),
        (
            relay_on("feishu", json!({"bot_id":"on_union"})),
            BOT_ID,
            OPEN_ID,
        ),
        (
            relay_on("discord", json!({"bot_id":"bad id"})),
            BOT_ID,
            "must contain 1..=128 letters, digits, '_', '-' or '.'",
        ),
        (
            relay_on("lark", json!({"trust_normalized":true})),
            "/sources/0/trust_normalized",
            "applies only to platforms without a raw-event verifier",
        ),
        (
            relay_source("admission", allowlist(json!([]))),
            ADMISSION,
            BOUNDED,
        ),
        (
            relay_source("admission", allowlist(json!(["same", "same"]))),
            ADMISSION,
            UNIQUE,
        ),
        (
            relay_source("admission", allowlist(json!([""]))),
            "/sources/0/admission/0",
            AT_LEAST_ONE,
        ),
        (
            relay_source("admission", allowlist(json!([long]))),
            "/sources/0/admission/0",
            AT_MOST_512,
        ),
        (
            relay_source("admission", scoped(listed(json!([])), open())),
            "/sources/0/admission/senders",
            BOUNDED,
        ),
        (
            relay_source("admission", scoped(open(), listed(json!(["same", "same"])))),
            "/sources/0/admission/chats",
            UNIQUE,
        ),
        (
            relay_source("admission", scoped(listed(json!([""])), open())),
            "/sources/0/admission/senders/0",
            AT_LEAST_ONE,
        ),
        // Custom.
        (
            custom_source("auth", json!({"type":"hmac","key_id":""})),
            "/sources/0/auth/key_id",
            AT_LEAST_ONE,
        ),
        (
            custom_source("auth", json!({"type":"hmac","key_id":long})),
            "/sources/0/auth/key_id",
            AT_MOST_512,
        ),
        (hashes(json!([])), HASHES, BOUNDED),
        (hashes(json!([hash(), hash()])), HASHES, UNIQUE),
        (
            hashes(json!([""])),
            "/sources/0/auth/key_hashes/0",
            AT_LEAST_ONE,
        ),
        (
            hashes(json!(["invalid"])),
            HASHES,
            "must be sha256: followed by lowercase hex",
        ),
        (
            custom_source("prompt_template", json!("x".repeat(8 * 1024 + 1))),
            "/sources/0/prompt_template",
            "must contain at most 8192 bytes",
        ),
        (
            custom_source("schema", json!({})),
            "/sources/0/schema",
            "must be null in v1",
        ),
        (
            sources(relay_channel(), json!([relay(), schema])),
            "/sources/1/schema",
            "must be null in v1",
        ),
        (attachments(json!({"pointer":""})), POINTER, AT_LEAST_ONE),
        (
            attachments(json!({"pointer":format!("/{}", "x".repeat(240))})),
            POINTER,
            "must contain at most 240 bytes",
        ),
        (
            attachments(json!({"pointer":"items"})),
            POINTER,
            "must be an RFC 6901 JSON pointer starting with '/' without control characters",
        ),
        (
            attachments(json!({"origins":origins})),
            "/sources/0/attachments/origins/16",
            "at most 16 origins are allowed",
        ),
        (
            attachments(json!({"origins":["http://cdn.example"]})),
            "/sources/0/attachments/origins/0",
            "must be an exact https origin such as https://host[:port]",
        ),
        (
            attachments(json!({"origins":["https://cdn.example","https://cdn.example"]})),
            "/sources/0/attachments/origins/1",
            "origins must be unique",
        ),
        // Partition.
        (
            edit(channel(), |value| value["partition"]["version"] = json!(2)),
            "/partition/version",
            "unsupported version",
        ),
        (partition(channel(), "conversation"), KEY, CUSTOM_KEY),
        (
            partition(channel(), "conversation_and_sender"),
            KEY,
            CUSTOM_KEY,
        ),
        (partition(channel(), "thread"), KEY, CUSTOM_KEY),
        (
            partition(sources(channel(), json!([relay(), custom()])), "thread"),
            KEY,
            CUSTOM_KEY,
        ),
        (
            partition(relay_channel(), "event_subject"),
            KEY,
            NEEDS_CUSTOM,
        ),
        (partition(relay_channel(), "per_event"), KEY, NEEDS_CUSTOM),
        (
            partition(telegram_channel(), "per_event"),
            KEY,
            NEEDS_CUSTOM,
        ),
        // The first error wins: the head, then sources, then partition, then sinks.
        (
            edit(sources(channel(), json!([])), |value| {
                value["profile_digest"] = json!(hash())
            }),
            "/profile_digest",
            "does not match profile",
        ),
        (
            edit(sources(channel(), json!([])), |value| {
                value["partition"]["version"] = json!(2)
            }),
            "/sources",
            COUNT,
        ),
        (
            edit(partition(channel(), "conversation"), |value| {
                value["reply"]["sinks"] = too_many_sinks;
            }),
            KEY,
            CUSTOM_KEY,
        ),
    ]
}
