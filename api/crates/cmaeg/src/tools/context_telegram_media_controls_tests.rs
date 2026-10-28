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
                    }
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
