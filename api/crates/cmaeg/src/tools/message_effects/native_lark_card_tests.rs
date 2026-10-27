use super::{native_lark_fixture::Fixture, *};
use crate::{
    interactions::{InteractionStore, PreparedCard},
    native_lark::callback::Callback,
};
use cmaeg_core::{channel::LarkRegion, seal::EventRef};
use std::sync::atomic::Ordering;
async fn card(f: &Fixture) -> (InteractionStore, PreparedCard) {
    let channel = f
        .app
        .state
        .resolver
        .resolve_current(f.app.event.channel_id.as_str())
        .await
        .unwrap();
    let context = crate::providers::EventContextSnapshot {
        schema_version: 1,
        channel_id: channel.id.clone(),
        conversation_id: f.app.event.conversation_id.clone(),
        activity: f.app.activity.clone(),
        event_ref: EventRef::seal(&f.app.event, &f.app.state.keys).unwrap(),
    };
    f.app.provider.memory.lock().unwrap().config.event_context =
        Some(serde_json::to_value(context).unwrap());
    let cards = InteractionStore::new(
        f.app.state.resolver.record_client(),
        f.app.state.keys.clone(),
        f.app.state.providers.clone(),
    );
    let mut card = cards
        .prepare_native_lark(
            &channel,
            &f.app.event,
            &crate::interactions::native_lark::fixture::outbound(false),
            false,
        )
        .await
        .unwrap();
    let confirmed = f
        .confirmed(false)
        .await
        .open(&f.app.state.keys)
        .unwrap()
        .payload;
    let a = confirmed.native_lark_effect.unwrap();
    let receipt = crate::native_lark::receipts::Store::new(
        f.app.state.resolver.record_client(),
        f.app.state.keys.clone(),
    )
    .load(
        &channel.id,
        &f.app.activity.source,
        &a.reply.message_id,
        now(),
    )
    .await
    .unwrap()
    .unwrap();
    cards.bind_native_lark(&mut card, receipt).await.unwrap();
    f.api.row.lock().unwrap()["msg_type"] = "interactive".into();
    (cards, card)
}
#[tokio::test]
async fn native_card_reaction_and_recall_share_identity_without_exposing_effect_reference() {
    for named in [false, true] {
        let mut f = Fixture::new(LarkRegion::Feishu, named, true).await;
        let (_, card) = card(&f).await;
        f.gates(false, false);
        let reference = card.card.reference.unwrap();
        assert!(reference
            .open(&f.app.state.keys)
            .unwrap()
            .payload
            .native_lark_effect
            .is_none());
        let caps = f
            .app
            .state
            .message_effect_capabilities(f.app.caller(), &reference)
            .await
            .unwrap();
        assert!(caps.delete && caps.reaction);
        assert_eq!(
            f.react(&reference, "react", Some("SMILE"))
                .await
                .unwrap()
                .delivery
                .kind(),
            "confirmed"
        );
        assert_eq!(
            f.react(&reference, "clear", None)
                .await
                .unwrap()
                .delivery
                .kind(),
            "confirmed"
        );
        assert_eq!(
            f.app
                .state
                .delete_message(f.app.caller(), &reference, "delete")
                .await
                .unwrap()
                .delivery
                .kind(),
            "confirmed"
        );
        assert_eq!(
            f.app
                .state
                .delete_message(f.app.caller(), &reference, "delete")
                .await
                .unwrap()
                .delivery
                .kind(),
            "confirmed"
        );
        assert!(f
            .react(&reference, "after_delete", Some("SMILE"))
            .await
            .is_err());
    }
}
#[tokio::test]
async fn native_recall_fences_callback_before_remote_write_and_uncertainty_never_reopens_it() {
    let f = Fixture::new(LarkRegion::Lark, false, true).await;
    let (cards, card) = card(&f).await;
    let reference = card.card.reference.as_ref().unwrap().clone();
    f.api.delay_ms.store(100, Ordering::SeqCst);
    f.api.uncertain.store(true, Ordering::SeqCst);
    let (outcome, refused) = tokio::join!(
        f.app
            .state
            .delete_message(f.app.caller(), &reference, "delete"),
        async {
            tokio::time::timeout(std::time::Duration::from_secs(2), async {
                loop {
                    if cards.load(&card.card.id).await.unwrap().0.superseded {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            let channel = f
                .app
                .state
                .resolver
                .resolve_current(f.app.event.channel_id.as_str())
                .await
                .unwrap();
            let a = card.card.native_lark.as_ref().unwrap();
            cards
                .consume_native_lark(
                    &channel,
                    &Callback {
                        event_id: "click_racing_recall".into(),
                        identity: a.reply().identity.clone(),
                        actor: f.app.activity.actor.id.clone(),
                        chat: f.app.activity.conversation.id.clone(),
                        message: "om_sent".into(),
                        locator: format!(
                            "cnl1:{}:1:0",
                            uuid::Uuid::parse_str(&card.card.id).unwrap().simple()
                        ),
                        form: None,
                        occurred_at: now(),
                    },
                )
                .await
        }
    );
    assert_eq!(outcome.unwrap().delivery.kind(), "unknown");
    assert!(refused.is_err());
    assert!(f
        .app
        .state
        .delete_message(f.app.caller(), &reference, "new_key")
        .await
        .is_err());
    assert_eq!(f.writes().len(), 1);
}
#[tokio::test]
async fn native_controls_discovery_is_separate_for_named_consumers_and_forms() {
    let mut f = Fixture::new(LarkRegion::Feishu, true, true).await;
    for forms in [false, true] {
        f.gates(false, false);
        f.app.state.native_lark_effects = Some(std::sync::Arc::new(
            NativeLarkEffectsClient::new(
                crate::native_lark::Client::production()
                    .unwrap()
                    .with_transport(f.api.clone()),
                false,
                false,
            )
            .with_controls(true, forms),
        ));
        let context = f.context().await;
        assert_eq!(context.capabilities.buttons, "native_card");
        assert_eq!(context.capabilities.forms, Some(forms));
        assert!(crate::mcp::catalog::view(&f.app.state)
            .iter()
            .any(|t| t.name == "setMessageReaction"));
        assert!(context.received_message.is_none());
    }
    assert!(f.writes().is_empty());
}
