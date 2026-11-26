//! Lark, Feishu and Discord keep Markdown punctuation literal (#2793), while
//! Telegram keeps its escaped dialect and every style keeps the safeguards.
use cmaeg::{
    delivery::render::{literal_platform, LiteralText, RelayText, Renderer},
    tools::Capabilities,
};
use cmaeg_core::{channel::Platform, message::OutboundMessage};
use serde_json::json;

const FIXTURES: [(&str, &str); 5] = [
    (
        "([nict.go.jp](https://www.nict.go.jp/en/sts/jst.html?utm_source=openai))",
        r"(\[nict.go.jp\](https://www.nict.go.jp/en/sts/jst.html?utm\_source=openai))",
    ),
    (
        "([chrono-ai.fun](https://chrono-ai.fun/zh/?utm_source=openai))",
        r"(\[chrono-ai.fun\](https://chrono-ai.fun/zh/?utm\_source=openai))",
    ),
    (
        "https://booking.example/test-link?a=b#share=_yNR5",
        r"https://booking.example/test-link?a=b#share=\_yNR5",
    ),
    ("literal _ * [ ]", r"literal \_ \* \[ \]"),
    (r"C:\qa\menu_01.txt", r"C:\\qa\\menu\_01.txt"),
];

fn message(text: &str) -> OutboundMessage {
    let message: OutboundMessage = serde_json::from_value(json!({ "text": text })).unwrap();
    message.validate().unwrap();
    message
}

fn literal_capabilities() -> [Capabilities; 5] {
    [
        Capabilities::native_lark(),
        Capabilities::native_discord(),
        Capabilities::relay(Platform::Lark),
        Capabilities::relay(Platform::Feishu),
        Capabilities::relay(Platform::Discord),
    ]
}

#[test]
fn literal_platforms_are_exactly_lark_feishu_and_discord() {
    for platform in [Platform::Lark, Platform::Feishu, Platform::Discord] {
        assert!(literal_platform(platform), "{platform:?}");
    }
    for platform in [Platform::Telegram, Platform::Slack, Platform::Whatsapp] {
        assert!(!literal_platform(platform), "{platform:?}");
    }
}

#[test]
fn issue_fixtures_keep_source_on_literal_platforms_and_escapes_on_telegram() {
    for (source, telegram) in FIXTURES {
        let message = message(source);
        for capabilities in literal_capabilities() {
            let (parts, _) = LiteralText.render(&message, &capabilities);
            assert_eq!(parts.len(), 1);
            assert_eq!(parts[0].text, source);
            assert_eq!(LiteralText.whole(&message, &capabilities).0.text, source);
        }
        let (parts, _) = RelayText.render(&message, &Capabilities::telegram());
        assert_eq!(parts[0].text, telegram);
        assert_eq!(
            RelayText.whole(&message, &Capabilities::telegram()).0.text,
            telegram
        );
    }
}

#[test]
fn literal_text_keeps_mention_control_and_bidi_safeguards() {
    let source = "hi @everyone <@123> <at user_id=\"x\"> \u{202e}x\u{2066}\u{0000}y\tz";
    let expected = "hi @\u{200b}everyone ‹@\u{200b}123› ‹at user_id=\"x\"› xy\tz";
    for capabilities in literal_capabilities() {
        let (parts, _) = LiteralText.render(&message(source), &capabilities);
        assert_eq!(parts[0].text, expected);
    }
}

#[test]
fn literal_urls_keep_at_signs_but_not_broadcast_mentions() {
    for (source, expected) in [
        (
            "see https://medium.com/@author/post and @author",
            "see https://medium.com/@author/post and @\u{200b}author",
        ),
        ("HTTPS://Example.com/@user", "HTTPS://Example.com/@user"),
        (
            "https://x.example/@everyone https://x.example/@here",
            "https://x.example/@\u{200b}everyone https://x.example/@\u{200b}here",
        ),
        (
            "https://a.example/x>@b <https://a.example/@c>",
            "https://a.example/x›@\u{200b}b ‹https://a.example/@c›",
        ),
        ("mailto:a@b.example", "mailto:a@\u{200b}b.example"),
    ] {
        let (parts, _) = LiteralText.render(&message(source), &Capabilities::native_discord());
        assert_eq!(parts[0].text, expected, "{source}");
    }
}

#[test]
fn literal_chunks_stay_within_budget_and_keep_every_character() {
    let source = format!(
        "{}\n\nhttps://example.com/a_b?c=[d]\n\n{}",
        "中文🦀_".repeat(900),
        "x*".repeat(1500)
    );
    let capabilities = Capabilities::native_discord();
    let (parts, report) = LiteralText.render(&message(&source), &capabilities);
    assert!(parts.len() > 1);
    assert!(parts
        .iter()
        .all(|part| part.text.encode_utf16().count() <= capabilities.max_text_chars as usize));
    assert_eq!(
        parts
            .iter()
            .map(|part| part.text.as_str())
            .collect::<String>(),
        source
    );
    let report = serde_json::to_value(report).unwrap();
    assert_eq!(
        report["blocks"][0]["outcome"]["how"],
        format!("chunked:{}", parts.len())
    );
}
