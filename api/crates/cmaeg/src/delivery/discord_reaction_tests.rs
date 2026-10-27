//! Real native send receipts are the sole authority for an outbound reaction target.
use super::{gate::recording::RecordingGate, intents::DeliveryOutcome, send::SendContext};
use crate::{
    discord::{reaction_tests, rest_fixture::Fixture},
    interactions::InteractionStore,
};
use cmaeg_core::message::OutboundMessage;
use serde_json::json;
use std::{sync::Arc, time::Duration};

#[tokio::test]
async fn discord_reactions_on_confirmed_split_replies_keep_only_the_sending_consumer() {
    let mut f = reaction_tests::setup().await;
    let mut definition = crate::channels::routing_fixture::routed();
    let reviewer = crate::channels::routing_fixture::REVIEWER;
    definition["routing"]["default_targets"] = json!(["default", reviewer]);
    let routed = crate::channels::routing_fixture::definition(definition);
    f.channel.routing = routed.routing;
    f.channel.secrets.consumer_keys = routed.secrets.consumer_keys;
    reaction_tests::sync_source(&mut f);
    f.message(|raw| raw["guild_id"] = "88".into()).await;
    assert_eq!(f.sweep(f.now()).await.pending, 0);
    let original = f
        .plans
        .recover_all_page(None, 32)
        .await
        .unwrap()
        .plans
        .remove(0);
    assert_eq!(original.targets.len(), 2);
    let selected = original
        .targets
        .iter()
        .find(|target| target.launch.consumer.as_str() == reviewer)
        .unwrap();
    let current = f
        .state
        .resolver
        .resolve_current(original.identity.channel_id.as_str())
        .await
        .unwrap();
    let discord = Fixture::new().await;
    for id in ["800", "801"] {
        discord.memory.responses.lock().unwrap().push_back((
            axum::http::StatusCode::OK,
            json!({"id":id,"channel_id":"99","author":{"id":"77"}}),
        ));
    }
    let (mut sender, _, _) = super::discord_tests::setup(&discord);
    let provider = crate::turns::routed::adapter_test_server::HttpServer::start().await;
    sender.relay = sender
        .relay
        .with_discord_reactions(true)
        .with_interactions(Arc::new(InteractionStore::new(
            f.fixture.records(),
            f.fixture.keys(),
            provider.client(true, Duration::from_secs(5)),
        )));
    let gate = RecordingGate::new(true);
    let trace = tracing::Span::none();
    let ctx = SendContext {
        channel: &current,
        trace: &trace,
        first_send_done: false,
        gate: &gate,
    };
    let message: OutboundMessage =
        serde_json::from_value(json!({"text":"😀".repeat(1001),"native_attachments":[{"kind":"file","source":{"type":"base64","data":"AQIDBA=="},"filename":"file.bin"}]})).unwrap();
    let receipt = sender.reply(&ctx, &selected.launch.event, &message).await.0;
    assert!(
        matches!(receipt.delivery, DeliveryOutcome::Confirmed { .. }),
        "{:?}",
        receipt.delivery
    );
    assert_eq!(discord.memory.calls.lock().unwrap().len(), 2);
    // Native per-route pacing uses real time; advance the deterministic inbox clock
    // beyond both actual confirmation timestamps before delivering the reactions.
    f.fixture
        .advance((time::OffsetDateTime::now_utc().unix_timestamp() - f.now()).max(0) * 1000);
    assert!(provider.memory.lock().unwrap().seen.is_empty());
    for (index, id) in ["800", "801"].into_iter().enumerate() {
        let mut reaction = reaction_tests::event(&f, "MESSAGE_REACTION_ADD", 3 + index as u64);
        reaction.data["message_id"] = id.into();
        reaction.data["message_author_id"] = "77".into();
        reaction_tests::persist(&f, &reaction, "gateway-a")
            .await
            .unwrap();
        reaction_tests::persist(&f, &reaction, "gateway-a")
            .await
            .unwrap();
    }
    assert_eq!(f.sweep(f.now()).await.pending, 0);
    let plans = f.plans.recover_all_page(None, 32).await.unwrap().plans;
    assert_eq!(plans.len(), 3, "plans: {plans:?}");
    for plan in plans
        .iter()
        .filter(|plan| plan.identity != original.identity)
    {
        assert_eq!(plan.targets.len(), 1);
        assert_eq!(plan.targets[0].launch.consumer, selected.launch.consumer);
        assert_eq!(plan.targets[0].launch.provider, selected.launch.provider);
        assert_eq!(
            plan.targets[0].launch.event.conversation_id,
            selected.launch.event.conversation_id
        );
        assert_eq!(
            plan.targets[0].launch.event.partition_key,
            selected.launch.event.partition_key
        );
        let cmaeg_core::activity::ActivityKind::Event { ce, .. } = &plan.activity.kind else {
            panic!()
        };
        assert_eq!(ce.data.as_ref().unwrap()["target_kind"], "outbound");
        assert_eq!(ce.data.as_ref().unwrap()["target_author"]["id"], "77");
    }
    assert_eq!(f.direct_calls(), 0);
}

#[tokio::test]
async fn discord_reaction_receipt_failure_is_unknown_and_unconfirmed_sends_grant_no_target() {
    for confirmed in [false, true] {
        let f = reaction_tests::setup().await;
        f.message(|raw| raw["guild_id"] = "88".into()).await;
        f.sweep(f.now()).await;
        let original = f
            .plans
            .recover_all_page(None, 32)
            .await
            .unwrap()
            .plans
            .remove(0);
        let current = f
            .state
            .resolver
            .resolve_current(original.identity.channel_id.as_str())
            .await
            .unwrap();
        let discord = Fixture::new().await;
        if !confirmed {
            *discord.memory.broken.lock().unwrap() = true;
        }
        let (mut sender, _, _) = super::discord_tests::setup(&discord);
        let provider = crate::turns::routed::adapter_test_server::HttpServer::start().await;
        sender.relay = sender
            .relay
            .with_discord_reactions(true)
            .with_interactions(Arc::new(InteractionStore::new(
                f.fixture.records(),
                f.fixture.keys(),
                provider.client(true, Duration::from_secs(5)),
            )));
        let id = crate::connections::store::index_id(
            "discord-confirmed-outbound-v1",
            &[&f.fixture.record, "77", "99", "800"],
        );
        if confirmed {
            f.fixture.fail(&id, false);
        }
        let gate = RecordingGate::new(true);
        let trace = tracing::Span::none();
        let ctx = SendContext {
            channel: &current,
            trace: &trace,
            first_send_done: false,
            gate: &gate,
        };
        let message: OutboundMessage = serde_json::from_value(json!({"text":"hello"})).unwrap();
        assert!(matches!(
            sender
                .reply(&ctx, &original.targets[0].launch.event, &message)
                .await
                .0
                .delivery,
            DeliveryOutcome::Unknown { .. }
        ));
        assert_eq!(discord.memory.calls.lock().unwrap().len(), 1);
        let mut reaction = reaction_tests::event(&f, "MESSAGE_REACTION_ADD", 2);
        reaction.data["message_id"] = "800".into();
        reaction_tests::persist(&f, &reaction, "gateway-a")
            .await
            .unwrap();
        f.sweep(f.now()).await;
        assert_eq!(
            f.plans
                .recover_all_page(None, 32)
                .await
                .unwrap()
                .plans
                .len(),
            1
        );
        assert_eq!(f.direct_calls(), 0);
    }
}
