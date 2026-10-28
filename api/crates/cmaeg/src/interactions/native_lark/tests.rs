use super::fixture::*;
use cmaeg_core::{
    activity::ActivityKind,
    channel::{LarkRegion, Secret},
    seal::ReplyAddress,
};
use serde_json::json;
use std::sync::Arc;

#[tokio::test]
async fn native_bound_cards_require_the_exact_stored_message_reference() {
    for case in 0..3 {
        let f = Fixture::new(false, false, LarkRegion::Lark).await;
        let mut card = f.card(false, true).await;
        let keys = crate::ingress::routing_plan::fixture::keys();
        if case == 0 {
            card.card.reference = None;
        } else {
            let mut reference = card
                .card
                .reference
                .as_ref()
                .unwrap()
                .open(&keys)
                .unwrap()
                .payload;
            if case == 1 {
                reference.platform_message_id = "om_foreign".into();
            } else {
                reference.interaction_record_id = Some(uuid::Uuid::new_v4().to_string());
            }
            card.card.reference =
                Some(cmaeg_core::ids::MessageRef::seal(&reference, &keys).unwrap());
        }
        f.store.save(&card.card, Some(card.version)).await.unwrap();
        assert!(f.store.load(&card.card.id).await.is_err());
        assert!(f
            .store
            .native_lark_current(&card.card, &f.channel)
            .await
            .is_err());
    }
}

#[tokio::test]
async fn recovery_skips_unreadable_and_foreign_records_without_starving_valid_clicks() {
    use crate::ingress::routing_store::test_server::Row;
    let f = Fixture::new(false, false, LarkRegion::Lark).await;
    let card = f.card(false, true).await;
    let callback = f.callback(&card, "recover_one", 0);
    f.store
        .consume_native_lark(&f.channel, &callback)
        .await
        .unwrap();
    for (id, plaintext) in [
        ("00000000-0000-4000-8000-000000000001", None),
        (
            "00000000-0000-4000-8000-000000000002",
            Some(b"malformed JSON".as_slice()),
        ),
    ] {
        let blob = plaintext.map_or_else(
            || vec![1],
            |bytes| {
                cmaeg_core::seal::seal_interaction(
                    &crate::ingress::routing_plan::fixture::keys(),
                    id,
                    bytes,
                )
                .unwrap()
            },
        );
        f.server.memory.rows.lock().unwrap().insert(
            id.into(),
            Row {
                owner: f.channel.definition.owner.subject.clone(),
                blob,
                expiry: now() + 86400,
                version: 1,
            },
        );
    }
    let page = f.store.native_lark_recovery_page(None).await.unwrap();
    assert!(page.cards.iter().any(|(id, _)| id == &card.card.id));
    f.server
        .memory
        .rows
        .lock()
        .unwrap()
        .get_mut(&card.card.id)
        .unwrap()
        .owner = "foreign_owner".into();
    let page = f.store.native_lark_recovery_page(None).await.unwrap();
    assert!(!page.cards.iter().any(|(id, _)| id == &card.card.id));
}

#[tokio::test]
async fn native_cards_keep_exact_default_named_thread_form_and_recoverable_click() {
    for named in [false, true] {
        for thread in [false, true] {
            for form in [false, true] {
                let f = Fixture::new(named, thread, LarkRegion::Feishu).await;
                let card = f.card(form, true).await;
                assert!(card.card.destination.is_none());
                assert!(card.card.lark.is_none());
                let click = f.callback(&card, "click_one", 0);
                assert!(card
                    .markup
                    .to_string()
                    .contains(if form { "cnlf1:" } else { "cnl1:" }));
                let id = f
                    .store
                    .consume_native_lark(&f.channel, &click)
                    .await
                    .unwrap();
                let pending = f.store.native_lark_pending(&id, &f.channel).await.unwrap();
                assert_eq!(pending.event.consumer, f.event.consumer);
                assert_eq!(pending.event.partition_key, f.event.partition_key);
                assert_eq!(pending.event.conversation_id, f.event.conversation_id);
                assert_eq!(pending.activity.conversation.thread_id.is_some(), thread);
                assert_eq!(pending.activity.routing.as_ref().unwrap().targets.len(), 1);
                assert_eq!(
                    f.store
                        .consume_native_lark(&f.channel, &click)
                        .await
                        .unwrap(),
                    id
                );
                let mut different = click.clone();
                different.event_id = "click_two".into();
                assert!(f
                    .store
                    .consume_native_lark(&f.channel, &different)
                    .await
                    .is_err());
                let ActivityKind::Interaction(result) = &pending.activity.kind else {
                    panic!()
                };
                if form {
                    assert_eq!(
                        result.values["fields"],
                        json!({"reason":"Approved","amount":12.5,"day":"2024-02-29","scope":"write"})
                    );
                }
                let page = f.store.native_lark_recovery_page(None).await.unwrap();
                assert!(page.cards.iter().any(|(card, _)| card == &id));
                super::current_dispatch(
                    &f.server.records,
                    &Arc::new(crate::ingress::routing_plan::fixture::keys()),
                    &f.channel,
                    &pending.activity,
                    &pending.event,
                )
                .await
                .unwrap();
                f.store.native_lark_admitted(&id).await.unwrap();
                assert!(!f
                    .store
                    .native_lark_recovery_page(None)
                    .await
                    .unwrap()
                    .cards
                    .iter()
                    .any(|(card, _)| card == &id));
            }
        }
    }
}

#[tokio::test]
async fn native_controls_refuse_unconfirmed_cross_transport_wrong_actor_message_revision_and_forms()
{
    for case in 0..10 {
        let f = Fixture::new(false, false, LarkRegion::Lark).await;
        let card = f.card(case == 8 || case == 9, case != 0).await;
        let mut click = f.callback(&card, "click_one", 0);
        match case {
            1 => click.actor = "ou_foreign".into(),
            2 => click.chat = "oc_foreign".into(),
            3 => click.message = "om_foreign".into(),
            4 => click.locator = click.locator.replace("cnl1:", "cl1:"),
            5 => click.locator = click.locator.replace(":1:", ":2:"),
            6 => click.identity.tenant_key = "foreign_tenant".into(),
            7 => click.occurred_at = card.card.expires_at,
            8 => {
                click
                    .form
                    .as_mut()
                    .unwrap()
                    .insert("extra".into(), json!("forged"));
            }
            9 => {
                click
                    .form
                    .as_mut()
                    .unwrap()
                    .insert("f1".into(), json!("NaN"));
            }
            _ => {}
        }
        assert!(
            f.store
                .consume_native_lark(&f.channel, &click)
                .await
                .is_err(),
            "case {case}"
        );
        assert!(f
            .store
            .load(&card.card.id)
            .await
            .unwrap()
            .0
            .consumed
            .is_none());
    }
}

#[tokio::test]
async fn source_key_consumer_credential_owner_and_routing_changes_revoke_pending_dispatch() {
    for named in [false, true] {
        for case in 0..9 {
            let mut f = Fixture::new(named, true, LarkRegion::Feishu).await;
            let card = f.card(false, true).await;
            let click = f.callback(&card, "click_one", 0);
            f.store
                .consume_native_lark(&f.channel, &click)
                .await
                .unwrap();
            let pending = f
                .store
                .native_lark_pending(&card.card.id, &f.channel)
                .await
                .unwrap();
            let c = Arc::make_mut(&mut f.channel);
            match case {
                0 => c.definition.sources.clear(),
                1 => {
                    c.definition
                        .secrets
                        .native_lark
                        .as_mut()
                        .unwrap()
                        .app_secret = Secret::new("fixture-rotated-app-secret")
                }
                2 => {
                    c.definition
                        .secrets
                        .native_lark
                        .as_mut()
                        .unwrap()
                        .encrypt_key = Secret::new("fixture-rotated-encrypt-key")
                }
                3 => {
                    c.definition
                        .secrets
                        .native_lark
                        .as_mut()
                        .unwrap()
                        .verification_token = Secret::new("fixture-rotated-verification")
                }
                4 => {
                    c.agent_key_hash = [7; 32];
                    for (_, hash) in &mut c.consumer_keys {
                        *hash = [7; 32];
                    }
                }
                5 => c.definition.owner.subject = "foreign_owner".into(),
                6 => c
                    .definition
                    .routing
                    .as_mut()
                    .unwrap()
                    .default_targets
                    .clear(),
                7 => {
                    if named {
                        c.definition.routing.as_mut().unwrap().consumers[0]
                            .provider
                            .binding_id = "other".parse().unwrap();
                    } else {
                        crate::channels::definition::fixture::replace_default_binding(
                            &mut c.definition,
                            "other".parse().unwrap(),
                        );
                    }
                }
                _ => {
                    if named {
                        c.definition.secrets.consumer_keys[0].1 =
                            Secret::new("fixture-rotated-provider");
                    } else {
                        crate::channels::definition::fixture::replace_default_credential(
                            &mut c.definition,
                            Some("fixture-rotated-provider"),
                        );
                    }
                }
            }
            assert!(
                f.store
                    .native_lark_pending(&card.card.id, &f.channel)
                    .await
                    .is_err(),
                "case {case} named {named}"
            );
            assert!(super::current_dispatch(
                &f.server.records,
                &Arc::new(crate::ingress::routing_plan::fixture::keys()),
                &f.channel,
                &pending.activity,
                &pending.event
            )
            .await
            .is_err());
        }
    }
}

#[tokio::test]
async fn concurrent_distinct_clicks_have_one_winner_and_replacement_never_extends_lifetime() {
    let f = Fixture::new(true, true, LarkRegion::Lark).await;
    let card = f.card(false, true).await;
    let one = f.callback(&card, "one", 0);
    let two = f.callback(&card, "two", 1);
    let (a, b) = tokio::join!(
        f.store.consume_native_lark(&f.channel, &one),
        f.store.consume_native_lark(&f.channel, &two)
    );
    assert_ne!(a.is_ok(), b.is_ok());
    let reference = card
        .card
        .reference
        .as_ref()
        .unwrap()
        .open(&crate::ingress::routing_plan::fixture::keys())
        .unwrap()
        .payload;
    assert!(f
        .store
        .replace_native_lark(&f.channel, &reference, &outbound(false))
        .await
        .is_err());
    assert!(f
        .store
        .fence_native_lark(&f.channel, &reference)
        .await
        .is_err());
    let admitted = f
        .store
        .native_lark_pending(&card.card.id, &f.channel)
        .await
        .unwrap();
    f.store.native_lark_admitted(&card.card.id).await.unwrap();
    let mut replacement = f
        .store
        .replace_native_lark(&f.channel, &reference, &outbound(false))
        .await
        .unwrap();
    assert_eq!(replacement.card.expires_at, card.card.expires_at);
    assert_eq!(replacement.card.consumer, card.card.consumer);
    let receipt = card
        .card
        .native_lark
        .as_ref()
        .unwrap()
        .destination
        .clone()
        .unwrap();
    assert!(f
        .store
        .bind_native_lark(&mut replacement, receipt.clone())
        .await
        .is_err());
    assert!(f.store.consume_native_lark(&f.channel, &one).await.is_err());
    f.store
        .native_lark_patch_confirmed(&mut replacement)
        .await
        .unwrap();
    f.store
        .bind_native_lark(&mut replacement, receipt)
        .await
        .unwrap();
    let pending = f.store.native_lark_pending(&card.card.id, &f.channel).await;
    assert!(pending.is_err());
    super::current_dispatch(
        &f.server.records,
        &Arc::new(crate::ingress::routing_plan::fixture::keys()),
        &f.channel,
        &admitted.activity,
        &admitted.event,
    )
    .await
    .unwrap();
    let ReplyAddress::NativeLark(original) = &f.event.reply else {
        panic!()
    };
    assert_eq!(
        replacement
            .card
            .native_lark
            .as_ref()
            .unwrap()
            .reply()
            .thread_id,
        original.thread_id
    );
}
