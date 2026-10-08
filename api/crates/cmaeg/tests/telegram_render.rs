use cmaeg::{
    delivery::render::{RelayText, Renderer},
    tools::Capabilities,
};
use cmaeg_core::{
    channel::Platform,
    message::{OutboundMessage, RenderingOutcome},
};
use serde_json::json;

fn render(value: serde_json::Value) -> (Vec<String>, serde_json::Value) {
    let message: OutboundMessage = serde_json::from_value(value).unwrap();
    message.validate().unwrap();
    let (parts, report) = RelayText.render(&message, &Capabilities::telegram());
    (
        parts.into_iter().map(|part| part.text).collect(),
        serde_json::to_value(report).unwrap(),
    )
}
#[test]
fn chunks_preserve_unicode_escape_atoms_and_paragraphs() {
    let (parts, report) = render(json!({"text": "a".repeat(9000)}));
    assert_eq!(
        parts.iter().map(String::len).collect::<Vec<_>>(),
        [4000, 4000, 1000]
    );
    assert_eq!(parts.concat(), "a".repeat(9000));
    assert_eq!(report["blocks"][0]["outcome"]["how"], "chunked:3");
    let (parts, _) = render(json!({"text": format!("{}🦀_*@\u{202e}<test>", "a".repeat(3999))}));
    assert!(parts.iter().all(|part| part.encode_utf16().count() <= 4000));
    assert_eq!(parts[1], "🦀\\_\\*@\u{200b}‹test›");
    let text = format!("{}\n\n{}", "a".repeat(2000), "b".repeat(3000));
    let (parts, _) = render(json!({"text": text}));
    assert_eq!(parts[0], format!("{}\n\n", "a".repeat(2000)));
    assert_eq!(parts.concat(), text);
    let (parts, _) = render(json!({"text": "\\_*`[]~|\u{2066}\u{0000}@"}));
    assert_eq!(parts[0], "\\\\\\_\\*\\`\\[\\]\\~\\|@\u{200b}");
}
#[test]
fn native_controls_render_separately_and_fallback_blocks_report_changes() {
    let (parts, report) = render(json!({"blocks":[
        {"type":"buttons","id":"buttons","options":[{"id":"yes","label":"Yes"},{"id":"no","label":"No"}]},
        {"type":"choice","id":"choice","prompt":"Choose","multi":true,"options":[{"id":"one","label":"One"}]},
        {"type":"card","title":"Title","body":"Body","fields":[{"label":"Name","value":"Value"}]},
        {"type":"form","id":"form","fields":[{"id":"name","label":"Name","kind":{"type":"text"},"required":true}]},
        {"type":"attachment","kind":"file","reference":"https://files.example/report.pdf","caption":"Report"},
        {"type":"link","url":"https://example.com/","label":"More"}
    ]}));
    assert_eq!(
        report["outcome"],
        serde_json::to_value(RenderingOutcome::Degraded).unwrap()
    );
    assert_eq!(report["blocks"][0]["outcome"]["type"], "exact");
    assert!(parts[0].starts_with("Choose"));
    for text in [
        "Name: Value",
        "Name: (required)",
        "\\[attachment: report.pdf\\]",
    ] {
        assert!(parts[0].contains(text), "missing {text}");
    }
}

fn labeled(
    value: serde_json::Value,
    capabilities: &Capabilities,
    label: Option<&str>,
) -> (Vec<String>, serde_json::Value) {
    let message: OutboundMessage = serde_json::from_value(value).unwrap();
    let (parts, report) = RelayText.render_labeled(&message, capabilities, label);
    (
        parts.into_iter().map(|part| part.text).collect(),
        serde_json::to_value(report).unwrap(),
    )
}

#[test]
fn a_label_prefix_is_escaped_identically_on_every_chat_transport() {
    // Relay Telegram, native Telegram, and a relay conversation of another
    // platform, as a default chat destination uses.
    for capabilities in [
        Capabilities::telegram(),
        Capabilities::native_telegram(),
        Capabilities::relay(Platform::Telegram),
        Capabilities::relay(Platform::Lark),
    ] {
        let (parts, _) = labeled(json!({"text":"Hi"}), &capabilities, Some("a_b*"));
        assert_eq!(parts, ["[a\\_b\\*] Hi"]);
    }
    let (parts, _) = labeled(
        json!({"text":"Hi"}),
        &Capabilities::telegram(),
        Some("<x@y>`[]~|\\\u{202e}"),
    );
    assert_eq!(parts, ["[‹x@\u{200b}y›\\`\\[\\]\\~\\|\\\\] Hi"]);
}

#[test]
fn a_label_opens_only_the_first_part_and_counts_toward_its_budget() {
    let label = "@".repeat(64);
    let prefix = format!("[{}] ", "@\u{200b}".repeat(64));
    let text = "a".repeat(9000);
    let (parts, report) = labeled(
        json!({ "text": text }),
        &Capabilities::telegram(),
        Some(&label),
    );
    assert!(parts[0].starts_with(&prefix));
    assert_eq!(
        parts[0].encode_utf16().count(),
        4000,
        "the prefix fills its share"
    );
    assert!(parts.iter().all(|part| part.encode_utf16().count() <= 4000));
    assert!(parts[1..].iter().all(|part| !part.starts_with('[')));
    assert_eq!(parts.concat(), format!("{prefix}{text}"));
    assert_eq!(report["blocks"][0]["outcome"]["how"], "chunked:3");
    // Without a label the same text fills its first part completely.
    let (parts, _) = labeled(json!({ "text": text }), &Capabilities::telegram(), None);
    assert_eq!(parts[0], "a".repeat(4000));
}

#[test]
fn a_label_never_shifts_the_rendering_report() {
    let message = json!({"text":"Pick","blocks":[
        {"type":"card","title":"Title","body":"Body"},
        {"type":"buttons","id":"buttons","options":[{"id":"yes","label":"Yes"},{"id":"no","label":"No"}]}
    ]});
    let (plain_parts, plain) = labeled(message.clone(), &Capabilities::telegram(), None);
    let (parts, report) = labeled(message.clone(), &Capabilities::telegram(), Some("lead"));
    assert_eq!(report, plain);
    assert_eq!(parts, [format!("[lead] {}", plain_parts[0])]);
    assert_eq!(
        (plain_parts, plain),
        render(message),
        "an unlabeled render is unchanged"
    );
}
