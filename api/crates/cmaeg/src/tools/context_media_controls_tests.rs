use super::media::NativeMediaCapabilities;
use crate::{
    discord::components_fixture::FixtureState, tools::message_effects::discord_card_fixture::state,
};

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
        assert!(editable.controls.unwrap().update);
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
