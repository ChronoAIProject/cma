use cmaeg::{
    delivery::render::{Renderer, Telegram},
    tools::Capabilities,
};
use cmaeg_core::message::{OutboundMessage, RenderingOutcome};
use serde_json::json;

fn render(value: serde_json::Value) -> (Vec<String>, serde_json::Value) {
    let message: OutboundMessage = serde_json::from_value(value).unwrap();
    message.validate().unwrap();
    let (parts, report) = Telegram.render(&message, &Capabilities::telegram());
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
