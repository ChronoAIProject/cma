use super::*;

#[tokio::test]
async fn discord_media_set_discovery_advertises_explicit_clear_only_with_its_prerequisites() {
    let f = crate::discord::components_fixture::FixtureState::named().await;
    let mut state = crate::tools::message_effects::discord_card_fixture::state(&f, true);
    state.native_media_enabled = true;
    state.discord_interactions_enabled = true;
    state.consumer_interactions_enabled = true;
    for issuance in [false, true] {
        for sets in [false, true] {
            for edits in [false, true] {
                for replace in [false, true] {
                    for verified in [false, true] {
                        state.native_discord_media_controls_enabled = issuance;
                        state.native_discord_media_card_sets_enabled = sets;
                        state.native_discord_media_card_edits_enabled = edits;
                        state.native_discord_media_card_replacements_enabled = replace;
                        let caps = NativeMediaCapabilities::for_event(
                            &state,
                            &f.channel,
                            &f.original,
                            &f.context.activity,
                            verified,
                        )
                        .unwrap();
                        assert_eq!(caps.controls.is_some(), issuance && verified);
                        if let Some(controls) = caps.controls {
                            assert_eq!(controls.max_attachments, if sets { 4 } else { 1 });
                            assert_eq!(
                                controls.clear_media,
                                (sets && edits && replace).then_some(true)
                            );
                            assert_eq!(controls.replace_media, (edits && replace).then_some(true));
                        }
                    }
                }
            }
        }
    }
}
