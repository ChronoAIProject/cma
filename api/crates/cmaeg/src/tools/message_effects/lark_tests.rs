use super::{lark_fixture::*, *};
use cmaeg_core::{
    channel::{Platform, Source},
    message_effect::{MessageReactionRequest, TelegramReaction},
};
use serde_json::json;
use std::sync::atomic::Ordering;

fn reaction(emoji: Option<&str>) -> MessageReactionRequest {
    MessageReactionRequest {
        reaction: emoji
            .map(|emoji| TelegramReaction::Emoji {
                emoji: emoji.into(),
            })
            .into_iter()
            .collect(),
        is_big: false,
    }
}
#[tokio::test]
async fn lark_and_feishu_confirmed_text_effects_use_existing_exact_proxy_and_replay() {
    for platform in [Platform::Lark, Platform::Feishu] {
        let mut f = App::new(platform).await;
        let reference = f.reference().await;
        let caps = f
            .state
            .message_effect_capabilities(f.caller(), &reference)
            .await
            .unwrap();
        assert!(caps.delete && caps.reaction);
        assert_eq!(
            f.state
                .delete_message(f.caller(), &reference, "delete-one")
                .await
                .unwrap()
                .delivery
                .kind(),
            "confirmed"
        );
        assert_eq!(f.writes().len(), 1);
        assert_eq!(f.writes()[0].0, "DELETE");
        assert_eq!(f.writes()[0].1, MESSAGE);
        f.restart();
        assert_eq!(
            f.state
                .delete_message(f.caller(), &reference, "delete-one")
                .await
                .unwrap()
                .delivery
                .kind(),
            "confirmed"
        );
        assert_eq!(f.writes().len(), 1);
        assert!(f
            .state
            .set_message_reaction(
                f.caller(),
                &reference,
                "delete-one",
                reaction(Some("THUMBSUP"))
            )
            .await
            .is_err());
    }
}
#[tokio::test]
async fn lark_reactions_replace_and_clear_only_retained_provider_identity_after_restart() {
    let mut f = App::new(Platform::Lark).await;
    let reference = f.reference().await;
    assert_eq!(
        f.state
            .set_message_reaction(f.caller(), &reference, "one", reaction(Some("THUMBSUP")))
            .await
            .unwrap()
            .delivery
            .kind(),
        "confirmed"
    );
    f.restart();
    assert_eq!(
        f.state
            .set_message_reaction(f.caller(), &reference, "two", reaction(Some("SMILE")))
            .await
            .unwrap()
            .delivery
            .kind(),
        "confirmed"
    );
    assert_eq!(
        f.state
            .set_message_reaction(f.caller(), &reference, "three", reaction(None))
            .await
            .unwrap()
            .delivery
            .kind(),
        "confirmed"
    );
    let writes = f.writes();
    assert_eq!(writes.len(), 4);
    assert_eq!(
        writes[0].2,
        json!({"reaction_type":{"emoji_type":"THUMBSUP"}})
    );
    assert!(writes[1].1.contains("/reactions/reaction_THUMBSUP?"));
    assert_eq!(writes[2].2, json!({"reaction_type":{"emoji_type":"SMILE"}}));
    assert!(writes[3].1.contains("/reactions/reaction_SMILE?"));
    assert_eq!(
        f.state
            .set_message_reaction(f.caller(), &reference, "four", reaction(None))
            .await
            .unwrap()
            .delivery
            .kind(),
        "confirmed"
    );
    assert_eq!(f.writes().len(), 4);
}
#[tokio::test]
async fn lark_unknown_write_and_new_keys_stay_fenced_across_restart() {
    for response in [json!({}), json!({"code":0,"data":{"reaction_id":"wrong"}})] {
        let mut f = App::new(Platform::Lark).await;
        let reference = f.reference().await;
        f.override_response("POST", REACTIONS, 200, response);
        assert_eq!(
            f.state
                .set_message_reaction(f.caller(), &reference, "one", reaction(Some("THUMBSUP")))
                .await
                .unwrap()
                .delivery
                .kind(),
            "unknown"
        );
        f.restart();
        for key in ["one", "different"] {
            assert_eq!(
                f.state
                    .set_message_reaction(f.caller(), &reference, key, reaction(Some("THUMBSUP")))
                    .await
                    .unwrap()
                    .delivery
                    .kind(),
                "unknown"
            );
        }
        assert_eq!(f.writes().len(), 1);
    }
}
#[tokio::test]
async fn lark_lost_final_persistence_never_repeats_mutation() {
    let mut f = App::new(Platform::Lark).await;
    let reference = f.reference().await;
    f.memory.fail_after_write.store(true, Ordering::SeqCst);
    assert_eq!(
        f.state
            .delete_message(f.caller(), &reference, "one")
            .await
            .unwrap()
            .delivery
            .kind(),
        "unknown"
    );
    f.memory.fail_after_write.store(false, Ordering::SeqCst);
    f.backend.fail_put.store(false, Ordering::SeqCst);
    f.restart();
    for key in ["one", "new"] {
        assert_eq!(
            f.state
                .delete_message(f.caller(), &reference, key)
                .await
                .unwrap()
                .delivery
                .kind(),
            "unknown"
        );
    }
    assert_eq!(f.writes().len(), 1);
}
#[tokio::test]
async fn lark_failed_claim_and_incorrect_live_bot_sender_or_chat_never_write() {
    let f = App::new(Platform::Lark).await;
    let reference = f.reference().await;
    f.backend.fail_put.store(true, Ordering::SeqCst);
    assert!(f
        .state
        .delete_message(f.caller(), &reference, "failed-store")
        .await
        .is_err());
    assert!(f.writes().is_empty());
    for (path, body) in [
        (
            "/api/v1/proxy/catalog-one/open-apis/bot/v3/info?_nyxid_via=connection-one",
            json!({"code":0,"bot":{"open_id":"ou_other"}}),
        ),
        (
            MESSAGE,
            json!({"code":0,"data":{"items":[{"message_id":"om_sent","chat_id":"oc_wrong","sender":{"sender_type":"app","id_type":"app_id","id":"cli_source"}}]}}),
        ),
        (
            MESSAGE,
            json!({"code":0,"data":{"items":[{"message_id":"om_sent","chat_id":"oc_chat","sender":{"sender_type":"user","id":"ou_source"}}]}}),
        ),
        (
            MESSAGE,
            json!({"code":0,"data":{"items":[{"message_id":"om_sent","chat_id":"oc_chat","thread_id":"omt_other","sender":{"sender_type":"app","id_type":"app_id","id":"cli_source"}}]}}),
        ),
    ] {
        let f = App::new(Platform::Lark).await;
        let reference = f.reference().await;
        f.override_response("GET", path, 200, body);
        assert_ne!(
            f.state
                .delete_message(f.caller(), &reference, "one")
                .await
                .unwrap()
                .delivery
                .kind(),
            "confirmed"
        );
        assert!(f.writes().is_empty());
    }
}
#[tokio::test]
async fn lark_source_owner_key_route_and_consumer_custody_is_current() {
    for field in ["owner", "key", "source", "provider", "route"] {
        let mut f = App::new(Platform::Lark).await;
        let reference = f.reference().await;
        match field {
            "owner" => f.definition.owner.subject = "other-owner".into(),
            "key" => {
                f.definition.secrets.channel_key = Some(cmaeg_core::channel::Secret::new("rotated"))
            }
            "provider" => f.definition.provider.binding_id = "changed-binding".parse().unwrap(),
            "source" => {
                if let Source::NyxidRelay {
                    lark_message_effects,
                    ..
                } = &mut f.definition.sources[0]
                {
                    *lark_message_effects = None;
                }
            }
            "route" => {
                if let Source::NyxidRelay { route_ids, .. } = &mut f.definition.sources[0] {
                    route_ids.clear();
                }
            }
            _ => unreachable!(),
        }
        f.replace();
        assert!(
            f.state
                .delete_message(f.caller(), &reference, "one")
                .await
                .is_err(),
            "{field}"
        );
        assert!(f.writes().is_empty());
    }
    let f = App::new(Platform::Lark).await;
    let reference = f.reference().await;
    assert!(f
        .state
        .delete_message(
            AgentCaller { key_hash: [8; 32] },
            &reference,
            "wrong-consumer"
        )
        .await
        .is_err());
    assert!(f.writes().is_empty());
}
#[tokio::test]
async fn lark_reference_expiry_wrong_id_and_request_shapes_refuse_before_transport() {
    let f = App::new(Platform::Lark).await;
    let reference = f.reference().await;
    let mut expired = reference.open(&f.state.keys).unwrap().payload;
    let a = expired.lark_effect.as_mut().unwrap();
    a.issued_at = now() - 100;
    a.expires_at = now() - 1;
    let expired = MessageRef::seal(&expired, &f.state.keys).unwrap();
    assert!(f
        .state
        .delete_message(f.caller(), &expired, "expired")
        .await
        .is_err());
    for request in [
        reaction(Some("👍")),
        MessageReactionRequest {
            is_big: true,
            ..reaction(None)
        },
        MessageReactionRequest {
            reaction: vec![TelegramReaction::CustomEmoji {
                custom_emoji_id: "123".into(),
            }],
            is_big: false,
        },
    ] {
        assert!(f
            .state
            .set_message_reaction(f.caller(), &reference, "bad", request)
            .await
            .is_err());
    }
    assert!(f.writes().is_empty());
}
#[tokio::test]
async fn lark_write_opt_in_is_separate_and_operations_are_independent() {
    for (delete, reaction_enabled) in [(true, false), (false, true)] {
        let mut f = App::new(Platform::Lark).await;
        if let Source::NyxidRelay {
            lark_message_effects: Some(binding),
            ..
        } = &mut f.definition.sources[0]
        {
            binding.delete = delete;
            binding.reaction = reaction_enabled;
        }
        f.replace();
        let reference = f.reference().await;
        let caps = f
            .state
            .message_effect_capabilities(f.caller(), &reference)
            .await
            .unwrap();
        assert_eq!((caps.delete, caps.reaction), (delete, reaction_enabled));
        let receipt = if delete {
            f.state
                .set_message_reaction(f.caller(), &reference, "disabled", reaction(None))
                .await
        } else {
            f.state
                .delete_message(f.caller(), &reference, "disabled")
                .await
        };
        assert_eq!(receipt.unwrap().delivery.kind(), "unsupported");
        assert!(f.writes().is_empty());
    }
    let mut f = App::new(Platform::Lark).await;
    if let Source::NyxidRelay {
        lark_message_effects,
        lark_resources,
        ..
    } = &mut f.definition.sources[0]
    {
        *lark_resources = Some(lark_message_effects.take().unwrap().connection);
    }
    f.replace();
    let receipt = f.send(json!({"text":"read authority only"})).await;
    assert!(receipt
        .message_ref
        .unwrap()
        .open(&f.state.keys)
        .unwrap()
        .payload
        .lark_effect
        .is_none());
}
#[tokio::test]
async fn lark_card_confirmed_and_split_or_unknown_text_does_not_grant_partial_authority() {
    let f = App::new(Platform::Lark).await;
    let card = f
        .send(json!({"blocks":[{"type":"card","title":"status","body":"ready"}]}))
        .await;
    assert_eq!(card.delivery.kind(), "confirmed");
    let reference = card.message_ref.unwrap();
    let payload = reference.open(&f.state.keys).unwrap().payload;
    assert!(payload.interaction_record_id.is_some());
    assert!(payload.lark_effect.is_some());
    assert_eq!(
        f.state
            .delete_message(f.caller(), &reference, "card")
            .await
            .unwrap()
            .delivery
            .kind(),
        "confirmed"
    );
    let f = App::new(Platform::Lark).await;
    let long = f.send(json!({"text":"x".repeat(5000)})).await;
    assert!(long
        .message_ref
        .unwrap()
        .open(&f.state.keys)
        .unwrap()
        .payload
        .lark_effect
        .is_none());
    f.override_response("POST", "/api/v1/channel-relay/reply", 500, json!({}));
    let unknown = f.send(json!({"text":"unknown"})).await;
    assert_eq!(unknown.delivery.kind(), "unknown");
    assert!(unknown.message_ref.is_none());
}

#[tokio::test]
async fn lark_different_keys_cannot_overlap_the_same_message() {
    let f = App::new(Platform::Lark).await;
    let reference = f.reference().await;
    f.memory.write_delay_ms.store(150, Ordering::SeqCst);
    let first = f.state.delete_message(f.caller(), &reference, "first");
    let second = async {
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while f.writes().is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        f.state
            .set_message_reaction(
                f.caller(),
                &reference,
                "different",
                reaction(Some("THUMBSUP")),
            )
            .await
    };
    let (first, second) = tokio::join!(first, second);
    assert_eq!(first.unwrap().delivery.kind(), "confirmed");
    assert_eq!(second.unwrap().delivery.kind(), "unknown");
    assert_eq!(f.writes().len(), 1);
}
#[tokio::test]
async fn lark_existing_threads_and_mismatched_original_activity_never_gain_authority() {
    let mut f = App::new(Platform::Lark).await;
    let channel = f
        .state
        .resolver
        .resolve_current(f.event.channel_id.as_str())
        .await
        .unwrap();
    assert!(lark_authority::event_supported(
        &channel,
        &f.event,
        &f.activity
    ));
    f.activity.conversation.thread_id = Some("omt_original".into());
    f.event.activity_sha256 = Some(cmaeg_core::seal::event_ref::activity_digest(&f.activity));
    assert!(!lark_authority::event_supported(
        &channel,
        &f.event,
        &f.activity
    ));
    f.activity.conversation.thread_id = None;
    assert!(!lark_authority::event_supported(
        &channel,
        &f.event,
        &f.activity
    ));
}
