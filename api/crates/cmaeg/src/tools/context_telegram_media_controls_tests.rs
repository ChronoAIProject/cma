use super::media::NativeMediaCapabilities;
use crate::{
    discord::components_fixture::FixtureState, ingress::routing_plan::fixture,
    tools::message_effects::discord_card_fixture::state,
};
use serde_json::json;

#[tokio::test]
async fn telegram_media_controls_discovery_matches_source_destination_and_issuance_gates() {
    let tools_fixture = FixtureState::new().await;
    let mut state = state(&tools_fixture, false);
    // Both adapters can be enabled in one gateway; Discord edit support must
    // not advertise edits for Telegram's issuance-only media cards.
    state.native_discord_media_controls_enabled = true;
    state.discord_interactions_enabled = true;
    state.native_discord_media_card_edits_enabled = true;
    state.native_discord_media_card_replacements_enabled = true;
    for named in [false, true] {
        let request = fixture::request(
            |value| {
                fixture::targets(value);
                value["sources"] =
                    json!([{"type":"telegram","bot_id":"77","admission":{"type":"all"}}]);
            },
            fixture::message("Original human event"),
        );
        let mut event = request
            .event_ref
            .open(&fixture::keys(), fixture::NOW)
            .unwrap()
            .payload;
        event.consumer = named.then(|| fixture::id(fixture::REVIEWER));
        for enabled in [false, true] {
            for consumer in [false, true] {
                for verified in [false, true] {
                    state.native_telegram_media_controls_enabled = enabled;
                    state.consumer_interactions_enabled = consumer;
                    let capabilities = NativeMediaCapabilities::for_event(
                        &state,
                        &request.channel,
                        &event,
                        &request.activity,
                        verified,
                    )
                    .unwrap();
                    assert_eq!(
                        capabilities.controls.is_some(),
                        enabled && verified && (!named || consumer)
                    );
                    if let Some(controls) = capabilities.controls {
                        assert_eq!(controls.max_attachments, 1);
                        assert!(controls.buttons && controls.choices);
                        assert_eq!(controls.caption_max_chars, 1024);
                        assert!(!controls.update);
                        assert!(controls.replace_media.is_none());
                    }
                    state.native_telegram_media_control_edits_enabled = true;
                    let editable = NativeMediaCapabilities::for_event(
                        &state,
                        &request.channel,
                        &event,
                        &request.activity,
                        verified,
                    )
                    .unwrap();
                    assert_eq!(
                        editable.controls.as_ref().map(|c| c.update),
                        (enabled && verified && (!named || consumer)).then_some(true)
                    );
                    assert!(editable
                        .controls
                        .as_ref()
                        .is_none_or(|controls| controls.replace_media.is_none()));
                    state.native_telegram_media_control_edits_enabled = false;
                }
            }
        }
        if let cmaeg_core::seal::ReplyAddress::Telegram { thread_id, .. } = &mut event.reply {
            *thread_id = Some("123".into());
        }
        assert!(NativeMediaCapabilities::for_event(
            &state,
            &request.channel,
            &event,
            &request.activity,
            true
        )
        .unwrap()
        .controls
        .is_none());
    }
}

#[tokio::test]
async fn telegram_album_discovery_is_gated_and_matches_original_native_destination() {
    let tools_fixture = FixtureState::new().await;
    let mut state = state(&tools_fixture, false);
    let request = fixture::request(
        |value| {
            fixture::targets(value);
            value["sources"] =
                json!([{"type":"telegram","bot_id":"77","admission":{"type":"all"}}]);
        },
        fixture::message("Original human event"),
    );
    let mut event = request
        .event_ref
        .open(&fixture::keys(), fixture::NOW)
        .unwrap()
        .payload;
    for enabled in [false, true] {
        for verified in [false, true] {
            state.native_telegram_albums_enabled = enabled;
            let value = NativeMediaCapabilities::for_event(
                &state,
                &request.channel,
                &event,
                &request.activity,
                verified,
            )
            .unwrap();
            assert_eq!(value.albums.is_some(), enabled && verified);
            if let Some(album) = value.albums {
                assert_eq!((album.min_attachments, album.max_attachments), (2, 4));
                assert_eq!(
                    album.kind_groups,
                    vec![vec!["image", "video"], vec!["file"], vec!["audio"]]
                );
                assert!(!album.controls && !album.message_operations);
            }
        }
    }
    if let cmaeg_core::seal::ReplyAddress::Telegram { chat_id, .. } = &mut event.reply {
        *chat_id = "-99".into();
    }
    assert!(NativeMediaCapabilities::for_event(
        &state,
        &request.channel,
        &event,
        &request.activity,
        true
    )
    .unwrap()
    .albums
    .is_none());
}

#[tokio::test]
async fn native_presentation_capabilities_are_opt_in_exact_destination_and_exclude_controls() {
    let tools_fixture = FixtureState::new().await;
    let mut state = state(&tools_fixture, false);
    let request = fixture::request(
        |value| {
            fixture::targets(value);
            value["sources"] =
                json!([{"type":"telegram","bot_id":"77","admission":{"type":"all"}}]);
        },
        fixture::message("Original human event"),
    );
    let mut event = request
        .event_ref
        .open(&fixture::keys(), fixture::NOW)
        .unwrap()
        .payload;
    state.native_telegram_media_controls_enabled = true;
    for enabled in [false, true] {
        state.native_media_presentations_enabled = enabled;
        for verified in [false, true] {
            let caps = NativeMediaCapabilities::for_event(
                &state,
                &request.channel,
                &event,
                &request.activity,
                verified,
            )
            .unwrap();
            assert_eq!(caps.presentation_profiles.is_some(), enabled && verified);
            assert_eq!(
                caps.kinds.iter().any(|kind| kind == "voice"),
                enabled && verified
            );
            if enabled && verified {
                assert_eq!(caps.presentation_profiles.as_ref().unwrap().len(), 3);
                assert!(caps
                    .presentation_profiles
                    .as_ref()
                    .unwrap()
                    .iter()
                    .all(|profile| !profile.message_operations));
                if let Some(controls) = caps.controls {
                    assert_eq!(
                        controls.kinds.unwrap(),
                        vec!["image", "file", "audio", "video"]
                    );
                }
            } else {
                assert!(serde_json::to_value(caps)
                    .unwrap()
                    .get("presentation_profiles")
                    .is_none());
            }
        }
    }
    if let cmaeg_core::seal::ReplyAddress::Telegram { chat_id, .. } = &mut event.reply {
        *chat_id = "999".into();
    }
    assert!(NativeMediaCapabilities::for_event(
        &state,
        &request.channel,
        &event,
        &request.activity,
        true
    )
    .unwrap()
    .presentation_profiles
    .is_none());
}
