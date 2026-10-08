use super::*;
use crate::{
    channels::routing_fixture::{definition, routed, REVIEWER},
    delivery::render::{RelayText, Renderer},
    tools::Capabilities,
};
use cmaeg_core::message::OutboundMessage;

fn rendered(text: String) -> String {
    let (parts, _) = RelayText.render(
        &OutboundMessage {
            text: Some(text),
            blocks: vec![],
            reply_to: None,
        },
        &Capabilities::native_telegram(),
    );
    assert_eq!(parts.len(), 1);
    parts[0].text.clone()
}

#[test]
fn notices_truncate_tokens_before_defanging_and_escape_only_once() {
    let def = definition(routed());
    let token = format!("{}TAIL", "é".repeat(32));
    let bounded = text(&def, "No {name}.", &RouteError::TargetUnknown(token)).unwrap();
    assert_eq!(bounded, format!("No {}.", "é".repeat(32)));
    for token in [
        "https://x.example",
        "x.example",
        "@SomeUser",
        "[label](https://x.example)",
    ] {
        let notice = text(&def, "No {name}.", &RouteError::TargetUnknown(token.into())).unwrap();
        let sent = rendered(notice);
        assert!(!sent.contains("https://"), "{sent}");
        assert!(!sent.contains("x.example"), "{sent}");
        assert!(!sent.contains("@SomeUser"), "{sent}");
        assert!(sent.contains("\\["), "{sent}");
        assert!(!sent.contains("\\\\["), "escaped twice: {sent}");
    }
}

#[test]
fn hidden_and_pending_names_never_enter_the_available_list() {
    let mut raw = routed();
    raw["routing"]["consumers"][0]["mentionable"] = false.into();
    raw["routing"]["consumers"][1]["pending"] = true.into();
    let def = definition(raw);
    let notice = text(
        &def,
        "No {name}. Available: {names}.",
        &RouteError::TargetNotMentionable(REVIEWER.into()),
    )
    .unwrap();
    assert!(notice.contains(REVIEWER));
    assert!(!notice.contains("reviewer"));
    assert!(!notice.contains("nyx"));
    assert!(notice.ends_with("Available: lead."));
}

#[test]
fn repeated_placeholders_are_bounded_on_utf8_boundaries_and_never_reexpanded() {
    let def = definition(routed());
    let template = "{name}".repeat(85);
    let notice = text(&def, &template, &RouteError::TargetUnknown("界".repeat(32))).unwrap();
    assert!(notice.len() <= 1024);
    assert!(notice.len() >= 1021);
    assert!(rendered(notice).encode_utf16().count() < 4000);
    let notice = text(
        &def,
        "No {name}. Available: {names}.",
        &RouteError::TargetUnknown("{names}".into()),
    )
    .unwrap();
    assert!(notice.starts_with("No {names}."));
    assert!(text(&def, "{name}", &RouteError::ConsumerUnavailable).is_none());
    assert!(text(&def, "{name}", &RouteError::KillSwitchClosed).is_none());
    assert_eq!(
        text(&def, "{name}", &RouteError::TooManyTargets).unwrap(),
        "Too many agents addressed; at most 4 per message."
    );
}
