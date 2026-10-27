use super::*;
use crate::channels::routing_fixture;
use cmaeg_core::{ids::ChannelId, seal::ChannelPointer};

#[test]
fn relay_text_edit_discovery_preserves_legacy_shape_and_limits_supported_platforms() {
    for platform in [
        Platform::Telegram,
        Platform::Lark,
        Platform::Feishu,
        Platform::Discord,
        Platform::Whatsapp,
        Platform::Slack,
    ] {
        let disabled = Capabilities::relay_with_text_edits(platform, false);
        assert_eq!(
            serde_json::to_value(disabled).unwrap(),
            serde_json::to_value(Capabilities::relay(platform)).unwrap()
        );
        let enabled = Capabilities::relay_with_text_edits(platform, true);
        assert_eq!(
            enabled.edit,
            matches!(
                platform,
                Platform::Telegram | Platform::Lark | Platform::Feishu | Platform::Discord
            )
        );
        assert_eq!(
            enabled.max_text_chars,
            if platform == Platform::Discord {
                2000
            } else {
                4000
            }
        );
        assert!(!enabled.typing);
        assert_eq!(
            enabled.buttons,
            if platform == Platform::Telegram {
                "inline_keyboard"
            } else {
                "numbered_text"
            }
        );
    }
    assert!(!Capabilities::custom().edit);
    assert!(Capabilities::native_telegram().edit);
}

#[test]
fn native_media_discovery_distinguishes_native_transports_from_attested_relays() {
    for platform in [
        Platform::Telegram,
        Platform::Lark,
        Platform::Feishu,
        Platform::Discord,
        Platform::Whatsapp,
    ] {
        let source = SourceKind::NyxidRelay {
            platform,
            route_id: "route".into(),
        };
        assert!(NativeMediaCapabilities::for_source(&source, false).is_none());
        let capability = NativeMediaCapabilities::for_source(&source, true).unwrap();
        assert_eq!(capability.kinds, ["image", "file", "audio", "video"]);
        assert_eq!(capability.sources, ["url", "base64"]);
        assert_eq!(capability.max_inline_bytes, 65536);
    }
    for source in [
        SourceKind::Custom,
        SourceKind::NyxidRelay {
            platform: Platform::Slack,
            route_id: "route".into(),
        },
    ] {
        assert!(NativeMediaCapabilities::for_source(&source, true).is_none());
    }
    for source in [
        SourceKind::Telegram {
            platform: Platform::Telegram,
            bot_id: "77".into(),
        },
        SourceKind::Discord {
            bot_id: "77".into(),
        },
    ] {
        for enabled in [false, true] {
            let capability = NativeMediaCapabilities::for_source(&source, enabled).unwrap();
            assert_eq!(capability.kinds, ["image", "file", "audio", "video"]);
            assert_eq!(capability.sources, ["url", "base64"]);
            assert_eq!(capability.max_attachments, 4);
            assert_eq!(capability.max_inline_bytes, 65536);
        }
    }
    assert!(serde_json::to_value(Capabilities::telegram())
        .unwrap()
        .get("native_media")
        .is_none());
}

fn resolved(value: serde_json::Value) -> ResolvedChannel {
    let definition = routing_fixture::definition(value);
    ResolvedChannel {
        id: ChannelId::new("cmaeg1.ch.context-fixture").unwrap(),
        pointer: ChannelPointer {
            v: 1,
            record_id: definition.record_id.clone(),
            record_url: "https://cma.example/api/v1/gateway/channel-records/record-1".into(),
        },
        definition,
        agent_key_hash: [7; 32],
        consumer_keys: Vec::new(),
        loaded_at: std::time::Instant::now(),
        version: 1,
    }
}

fn id(value: &str) -> ConsumerId {
    ConsumerId::parse(value).unwrap()
}

#[test]
fn a_routed_channel_summarizes_the_calling_consumer() {
    let channel = resolved(routing_fixture::routed());
    let summarize = |consumer: &ConsumerId| {
        summary(&channel, &consumer_view(&channel, consumer).unwrap()).unwrap()
    };
    let lead = summarize(&ConsumerId::default_consumer());
    assert_eq!(
        (lead.id.as_str(), lead.name.as_str(), lead.label.as_str()),
        ("default", "lead", "lead")
    );
    let reviewer = summarize(&id(routing_fixture::REVIEWER));
    assert_eq!(
        (
            reviewer.id.as_str(),
            reviewer.name.as_str(),
            reviewer.label.as_str()
        ),
        (routing_fixture::REVIEWER, "reviewer", "reviewer")
    );
}

#[test]
fn an_unrouted_channel_has_no_summary() {
    let mut value = routing_fixture::routed();
    value.as_object_mut().unwrap().remove("routing");
    let channel = resolved(value);
    let default = consumer_view(&channel, &ConsumerId::default_consumer()).unwrap();
    assert_eq!(summary(&channel, &default), None);
}

#[test]
fn a_removed_consumer_is_answered_as_an_expired_reference() {
    let channel = resolved(routing_fixture::routed());
    let removed = consumer_view(&channel, &id("csm_0000000000ff")).unwrap_err();
    assert_eq!(
        (removed.code, removed.status, removed.detail.as_deref()),
        (ProblemCode::EventRefExpired, 410, Some("consumer_removed"))
    );
}

#[test]
fn native_whatsapp_media_discovery_is_independent_and_reports_exact_subset() {
    assert!(NativeMediaCapabilities::native_whatsapp(false).is_none());
    let capability = NativeMediaCapabilities::native_whatsapp(true).unwrap();
    assert_eq!(capability.kinds, ["image", "file"]);
    assert_eq!(
        capability.mime_types.unwrap(),
        ["image/jpeg", "image/png", "application/pdf"]
    );
    assert_eq!(capability.max_image_bytes, Some(5_000_000));
    assert_eq!(capability.max_total_bytes, Some(8 * 1024 * 1024));
    assert_eq!(capability.max_caption_chars, Some(1024));
    assert_eq!(capability.max_attachments, 4);
    assert_eq!(capability.max_inline_bytes, 65536);
}
