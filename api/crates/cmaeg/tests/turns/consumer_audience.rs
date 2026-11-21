//! A routed turn renders its attachment block for the consumer serving it:
//! on a channel whose default is a `cma_codex` profile binding, a `nyxbot`
//! reviewer's turn carries the no-tools descriptor.
use super::{
    preparation_support::{voice, Media, Speech, WORDS},
    routing_fixture,
};
use cmaeg_core::{
    channel::{ChannelDefinition, Secret},
    ids::ConsumerId,
};
use serde_json::json;
use std::time::Duration;

fn reviewer() -> ConsumerId {
    ConsumerId::parse(routing_fixture::REVIEWER).unwrap()
}

/// Add `reviewer`, a `nyxbot` consumer with its own binding and key.
fn add_nyxbot_reviewer(definition: &mut ChannelDefinition) {
    let consumer = routing_fixture::consumer(routing_fixture::REVIEWER, "reviewer", "nyxbot");
    definition.routing = Some(
        serde_json::from_value(json!({"version": 1, "default_consumer": {"name": "lead"},
            "consumers": [consumer], "default_targets": ["default"]}))
        .unwrap(),
    );
    let key = routing_fixture::agent_key(routing_fixture::REVIEWER);
    definition.secrets.consumer_keys = vec![(reviewer(), Secret::new(key))];
}

#[tokio::test]
async fn each_consumer_turn_renders_attachments_for_its_own_audience() {
    let media = Media::new(Speech::Words, Duration::from_secs(5)).await;
    let items = vec![voice(&media, 0, Some(4))];
    let lead = media.turn("lead-turn", items.clone(), add_nyxbot_reviewer);
    let input = media.input(lead).await;
    assert!(input.contains("readEventAttachment"), "{input}");
    let mut turn = media.turn("reviewer-turn", items, add_nyxbot_reviewer);
    turn.consumer = Some(reviewer());
    let input = media.input(turn).await;
    // The transcript is inline for every audience; the tool hint is not.
    assert!(input.contains(WORDS), "{input}");
    assert!(!input.contains("readEventAttachment"), "{input}");
}
