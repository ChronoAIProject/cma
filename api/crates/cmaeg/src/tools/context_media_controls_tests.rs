use super::media::NativeMediaCapabilities;
use crate::{
    discord::components_fixture::FixtureState, tools::message_effects::discord_card_fixture::state,
};

#[tokio::test]
async fn discord_voice_profile_requires_independent_gate_and_exact_verified_destination() {
    let f = FixtureState::new().await;
    let mut state = state(&f, false);
    state.native_media_presentations_enabled = true;
    for enabled in [false, true] {
        for verified in [false, true] {
            state.native_discord_voice_enabled = enabled;
            let view = NativeMediaCapabilities::for_event(
                &state,
                &f.channel,
                &f.original,
                &f.context.activity,
                verified,
            )
            .unwrap();
            assert_eq!(view.presentation_profiles.is_some(), enabled && verified);
            assert_eq!(
                view.kinds.iter().any(|kind| kind == "voice"),
                enabled && verified
            );
            if let Some(profiles) = view.presentation_profiles {
                assert_eq!(profiles.len(), 1);
                assert_eq!(profiles[0].mime_types, ["audio/wav"]);
                assert_eq!(profiles[0].max_bytes, 3_840_044);
                assert_eq!(profiles[0].max_attachments, Some(1));
                assert_eq!(profiles[0].text, Some(false));
                assert!(!profiles[0].caption && !profiles[0].message_operations);
            }
        }
    }
    let mut event = f.original.clone();
    if let cmaeg_core::seal::ReplyAddress::Discord { channel_id, .. } = &mut event.reply {
        *channel_id = "100".into();
    }
    let view =
        NativeMediaCapabilities::for_event(&state, &f.channel, &event, &f.context.activity, true)
            .unwrap();
    assert!(view.presentation_profiles.is_none());
}

#[tokio::test]
async fn discord_media_controls_discovery_requires_verified_source_and_each_issuance_gate() {
    for named in [false, true] {
        let f = if named {
            FixtureState::named().await
        } else {
            FixtureState::new().await
        };
        let mut state = state(&f, false);
        state.native_telegram_media_controls_enabled = true;
        state.native_telegram_media_control_edits_enabled = true;
        for media in [false, true] {
            for controls in [false, true] {
                for consumer in [false, true] {
                    for verified in [false, true] {
                        state.native_discord_media_controls_enabled = media;
                        state.discord_interactions_enabled = controls;
                        state.consumer_interactions_enabled = consumer;
                        let view = NativeMediaCapabilities::for_event(
                            &state,
                            &f.channel,
                            &f.original,
                            &f.context.activity,
                            verified,
                        )
                        .unwrap();
                        assert_eq!(
                            view.controls.is_some(),
                            media && controls && verified && (!named || consumer)
                        );
                        if let Some(supported) = view.controls {
                            assert_eq!(supported.max_attachments, 1);
                            assert!(supported.buttons && supported.choices);
                            assert_eq!(supported.caption_max_chars, 1024);
                            assert!(!supported.update);
                            assert!(supported.replace_media.is_none());
                        }
                    }
                }
            }
        }
        state.native_discord_media_card_edits_enabled = true;
        let editable = NativeMediaCapabilities::for_event(
            &state,
            &f.channel,
            &f.original,
            &f.context.activity,
            true,
        )
        .unwrap();
        let controls = editable.controls.unwrap();
        assert!(controls.update);
        assert!(controls.replace_media.is_none());
        state.native_discord_media_card_replacements_enabled = true;
        let replacement = NativeMediaCapabilities::for_event(
            &state,
            &f.channel,
            &f.original,
            &f.context.activity,
            true,
        )
        .unwrap();
        assert_eq!(replacement.controls.unwrap().replace_media, Some(true));
        let mut event = f.original.clone();
        if let cmaeg_core::seal::ReplyAddress::Discord { channel_id, .. } = &mut event.reply {
            *channel_id = "100".into();
        }
        let view = NativeMediaCapabilities::for_event(
            &state,
            &f.channel,
            &event,
            &f.context.activity,
            true,
        )
        .unwrap();
        assert!(view.controls.is_none());
    }
}
