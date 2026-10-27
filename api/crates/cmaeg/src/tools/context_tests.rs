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
    assert_eq!(
        capability.kinds,
        ["image", "file", "audio", "sticker", "video"]
    );
    assert_eq!(
        capability.mime_types.unwrap(),
        [
            "image/jpeg",
            "image/png",
            "application/pdf",
            "text/plain",
            "application/msword",
            "application/vnd.ms-excel",
            "application/vnd.ms-powerpoint",
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            "application/vnd.openxmlformats-officedocument.presentationml.presentation",
            "audio/aac",
            "audio/amr",
            "audio/mpeg",
            "audio/mp4",
            "audio/ogg",
            "image/webp",
            "video/mp4",
            "video/3gpp"
        ]
    );
    assert_eq!(capability.max_image_bytes, Some(5_000_000));
    assert_eq!(capability.max_total_bytes, Some(8 * 1024 * 1024));
    assert_eq!(capability.max_caption_chars, Some(1024));
    assert_eq!(
        serde_json::to_value(capability.video.unwrap()).unwrap(),
        serde_json::json!({"video_codec": "h264", "video_profile": "baseline", "audio_codec": "aac_lc", "max_audio_tracks": 1, "max_width": 1920, "max_height": 1080, "max_frame_rate": 60, "max_duration_ms": 600000, "max_samples": 65536, "progressive_only": true, "max_slices_per_frame": 1, "fast_start_required": true})
    );
    let sticker = capability.sticker.unwrap();
    assert_eq!((sticker.width, sticker.height), (512, 512));
    assert_eq!(
        (sticker.max_static_bytes, sticker.max_animated_bytes),
        (100_000, 500_000)
    );
    assert_eq!(
        (
            sticker.max_frames,
            sticker.max_duration_ms,
            sticker.min_frame_duration_ms
        ),
        (128, 10_000, 8)
    );
    assert_eq!(capability.max_attachments, 4);
    assert_eq!(capability.max_inline_bytes, 65536);
}

#[test]
fn native_whatsapp_template_discovery_requires_verified_private_destination_and_current_custody() {
    use crate::{
        ingress::routing_plan::fixture,
        whatsapp::tests::{credentials, source},
    };
    use cmaeg_core::{activity::ConversationKind, channel::Secret, seal::ReplyAddress};
    for defect in 0..11 {
        let mut request = fixture::request(|_| {}, fixture::message("invoice please"));
        let channel = std::sync::Arc::make_mut(&mut request.channel);
        channel.definition.sources = vec![source()];
        channel.definition.secrets.whatsapp = Some(credentials());
        request.activity.source = SourceKind::Whatsapp {
            app_id: "12345".into(),
            waba_id: "23456".into(),
            phone_number_id: "34567".into(),
        };
        request.activity.conversation.id = "447700900123".into();
        let mut event = request
            .event_ref
            .open(&fixture::keys(), fixture::NOW)
            .unwrap()
            .payload;
        event.reply = ReplyAddress::Whatsapp {
            app_id: "12345".into(),
            waba_id: "23456".into(),
            phone_number_id: "34567".into(),
            recipient_id: "447700900123".into(),
            message_id: "wamid.original".into(),
        };
        match defect {
            0..=2 => {}
            3 => channel.definition.sources.clear(),
            4 => channel.definition.secrets.whatsapp = None,
            5 => {
                channel
                    .definition
                    .secrets
                    .whatsapp
                    .as_mut()
                    .unwrap()
                    .access_token = Secret::new("invalid")
            }
            6 => request.activity.source = SourceKind::Custom,
            7 => {
                if let SourceKind::Whatsapp {
                    phone_number_id, ..
                } = &mut request.activity.source
                {
                    *phone_number_id = "99999".into();
                }
            }
            8 => request.activity.conversation.id = "99999".into(),
            9 => request.activity.conversation.kind = ConversationKind::Group,
            10 => request.activity.conversation.thread_id = Some("thread".into()),
            _ => unreachable!(),
        }
        assert_eq!(
            native_whatsapp_templates(defect != 1, channel, &event, &request.activity, defect != 2),
            defect == 0,
            "{defect}"
        );
    }
}

#[test]
fn native_whatsapp_group_discovery_retains_exact_participant_and_supported_subset() {
    use crate::{
        ingress::routing_plan::fixture,
        whatsapp::{
            groups::tests::{reply, source_group, GROUP},
            tests::credentials,
        },
    };
    use cmaeg_core::{activity::ConversationKind, seal::ReplyAddress};
    let mut request = fixture::request(|_| {}, fixture::message("hello"));
    let channel = std::sync::Arc::make_mut(&mut request.channel);
    channel.definition.sources = vec![source_group()];
    channel.definition.secrets.whatsapp = Some(credentials());
    request.activity.source = SourceKind::Whatsapp {
        app_id: "12345".into(),
        waba_id: "23456".into(),
        phone_number_id: "34567".into(),
    };
    request.activity.actor.id = "447700900123".into();
    request.activity.conversation.id = GROUP.into();
    request.activity.conversation.kind = ConversationKind::Group;
    let mut event = request
        .event_ref
        .open(&fixture::keys(), fixture::NOW)
        .unwrap()
        .payload;
    event.reply = ReplyAddress::WhatsappGroup(reply());
    assert!(native_whatsapp_templates(
        true,
        channel,
        &event,
        &request.activity,
        true
    ));
    assert!(!native_whatsapp_templates(
        false,
        channel,
        &event,
        &request.activity,
        true
    ));
    assert!(!native_whatsapp_templates(
        true,
        channel,
        &event,
        &request.activity,
        false
    ));
    request.activity.actor.id = "447700900999".into();
    assert!(!native_whatsapp_templates(
        true,
        channel,
        &event,
        &request.activity,
        true
    ));
    let media = NativeMediaCapabilities::native_whatsapp_group(true).unwrap();
    assert_eq!(media.kinds, vec!["image", "file", "audio", "video"]);
    assert!(media.sticker.is_none());
    assert!(media.video.is_some());
    assert!(!media.mime_types.unwrap().contains(&"image/webp".to_owned()));
}

#[test]
fn native_lark_media_discovery_is_independent_and_reports_bounded_subset() {
    assert!(NativeMediaCapabilities::native_lark(false).is_none());
    let source = SourceKind::NativeLark(crate::native_lark::test_support::identity());
    assert!(NativeMediaCapabilities::for_source(&source, true).is_none());
    let capability = NativeMediaCapabilities::native_lark(true).unwrap();
    assert_eq!(capability.kinds, ["image", "file", "audio", "video"]);
    assert_eq!(
        capability.mime_types.unwrap(),
        [
            "image/jpeg",
            "image/png",
            "application/pdf",
            "text/plain",
            "audio/ogg",
            "video/mp4"
        ]
    );
    assert_eq!(capability.max_attachments, 4);
    assert_eq!(capability.max_inline_bytes, 65536);
    assert_eq!(capability.max_total_bytes, Some(8 * 1024 * 1024));
    assert_eq!(capability.max_image_bytes, Some(8 * 1024 * 1024));
    assert_eq!(capability.max_caption_chars, Some(0));
    assert!(capability.sticker.is_none());
    assert_eq!(capability.video.unwrap().max_duration_ms, 600_000);
}
