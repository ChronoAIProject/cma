use super::{discord_card_fixture::*, *};
use crate::discord::components_fixture::{message, FixtureState};
use axum::http::StatusCode;
use cmaeg_core::seal::seal_blob;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

async fn claim_id(f: &FixtureState, card: &crate::interactions::PreparedCard, key: &str) -> String {
    let reference = reference(card).open(&f.keys()).unwrap().payload;
    let scope = f
        .store
        .discord_card_candidate(&f.channel, &reference)
        .await
        .unwrap()
        .effect_scope();
    let digest = Sha256::digest(format!("cmaeg.discord-card-effect.v1\0{scope}\0{key}"));
    let mut bytes: [u8; 16] = digest[..16].try_into().unwrap();
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    uuid::Uuid::from_bytes(bytes).to_string()
}
fn revoked(f: &FixtureState, case: &str) -> Value {
    let mut value = f.setup.channel.to_blob_json();
    match case {
        "provider" => value["routing"]["consumers"][0]["provider"]["binding_id"] = "rebound".into(),
        "consumer" => {
            value["routing"]["consumers"] = json!([]);
            value["secrets"]["consumer_keys"] = json!([]);
        }
        "key" => {
            let key = "nyx_fixture_rotated_card_owner";
            value["routing"]["consumers"][0]["agent_key_hash"] =
                crate::channels::routing_fixture::hash(key).into();
            value["secrets"]["consumer_keys"][0][1] = key.into();
        }
        "pending" => {
            value["routing"]["consumers"][0]["pending"] = true.into();
        }
        "token" => value["secrets"]["discord_bot_token"] = "rotated-bot-token".into(),
        "owner" => value["owner"]["subject"] = "new-owner".into(),
        _ => value["sources"][0]["admission"] = json!({"type":"allowlist","sender_ids":["43"]}),
    }
    value
}
#[tokio::test]
async fn card_effects_recheck_revocation_before_claim_and_after_claim_before_dispatch() {
    for after_claim in [false, true] {
        for case in [
            "provider", "consumer", "key", "token", "owner", "source", "pending",
        ] {
            let f = FixtureState::named().await;
            let card = f.card(&message(false)).await;
            let reference = reference(&card);
            let tools = state(&f, true);
            echo(&f, StatusCode::NO_CONTENT, json!([]));
            let changed = revoked(&f, case);
            let blob = seal_blob(&f.keys(), &serde_json::to_vec(&changed).unwrap()).unwrap();
            if after_claim {
                let memory = f.setup.fixture.memory.clone();
                let prior = f.rest.memory.on_call.lock().unwrap().clone().unwrap();
                *f.rest.memory.on_call.lock().unwrap() = Some(Arc::new(move |call| {
                    prior(call);
                    if call.path == "/api/v10/channels/99" {
                        *memory.channel.lock().unwrap() = blob.clone();
                    }
                }));
            } else {
                *f.setup.fixture.memory.channel.lock().unwrap() = blob;
            }
            let result = tools
                .delete_message(caller(&f), &reference, "revoked")
                .await;
            assert!(result.is_err(), "{case} after claim={after_claim}");
            assert!(writes(&f).is_empty(), "{case}");
        }
    }
}

#[tokio::test]
async fn card_effects_revoke_saved_replay_and_do_not_retarget_routing_or_partition() {
    let f = FixtureState::named_thread(11).await;
    let card = f.card(&message(false)).await;
    let reference = reference(&card);
    let tools = state(&f, true);
    echo(&f, StatusCode::NO_CONTENT, json!([]));
    let mut changed = f.setup.channel.to_blob_json();
    changed["routing"]["default_targets"] = json!(["default"]);
    changed["partition"]["key"] = "fixed".into();
    f.setup
        .fixture
        .replace(&serde_json::from_value(changed).unwrap());
    let first = tools
        .delete_message(caller(&f), &reference, "done")
        .await
        .unwrap();
    assert_eq!(first.delivery.kind(), "confirmed");
    let saved = serde_json::to_value(f.store.load(&card.card.id).await.unwrap().0).unwrap();
    let original = serde_json::to_value(&card.card).unwrap();
    for field in [
        "actor",
        "chat",
        "thread",
        "event_id",
        "route_id",
        "consumer",
        "discord",
        "expires_at",
    ] {
        assert_eq!(saved[field], original[field], "{field}");
    }
    f.setup
        .fixture
        .replace(&serde_json::from_value(revoked(&f, "provider")).unwrap());
    assert!(tools
        .delete_message(caller(&f), &reference, "done")
        .await
        .is_err());
    assert_eq!(writes(&f).len(), 1);
}

#[tokio::test]
async fn card_effects_lost_claim_and_delete_cas_ack_never_repeat_dispatch() {
    for claim_fault in [true, false] {
        let f = FixtureState::new().await;
        let card = f.card(&message(false)).await;
        let reference = reference(&card);
        let tools = state(&f, true);
        let id = if claim_fault {
            claim_id(&f, &card, "lost").await
        } else {
            card.card.id.clone()
        };
        f.setup.fixture.fail(&id, true);
        let first = tools.delete_message(caller(&f), &reference, "lost").await;
        assert!(first.is_err() || first.unwrap().delivery.kind() == "unknown");
        let retry = state(&f, false)
            .delete_message(caller(&f), &reference, "lost")
            .await
            .unwrap();
        assert_eq!(retry.delivery.kind(), "unknown");
        assert!(writes(&f).is_empty());
    }
}

#[tokio::test]
async fn card_effects_lost_final_card_or_receipt_ack_keep_destination_fenced() {
    for card_save in [true, false] {
        for after_commit in [false, true] {
            let f = FixtureState::new().await;
            let card = f.card(&message(false)).await;
            let reference = reference(&card);
            let mut tools = state(&f, true);
            let _engine = engine(&f, &mut tools);
            echo(&f, StatusCode::NO_CONTENT, json!([]));
            let target = if card_save {
                card.card.id.clone()
            } else {
                claim_id(&f, &card, "lost").await
            };
            let memory = f.setup.fixture.memory.clone();
            let prior = f.rest.memory.on_call.lock().unwrap().clone().unwrap();
            *f.rest.memory.on_call.lock().unwrap() = Some(Arc::new(move |call| {
                prior(call);
                if call.method == "DELETE" {
                    *memory.fault.lock().unwrap() = Some((target.clone(), after_commit));
                }
            }));
            assert_eq!(
                tools
                    .delete_message(caller(&f), &reference, "lost")
                    .await
                    .unwrap()
                    .delivery
                    .kind(),
                "unknown"
            );
            let retry = tools
                .delete_message(caller(&f), &reference, "lost")
                .await
                .unwrap();
            assert_eq!(
                retry.delivery.kind(),
                if !card_save && after_commit {
                    "confirmed"
                } else {
                    "unknown"
                }
            );
            assert_eq!(
                tools
                    .delete_message(caller(&f), &reference, "new")
                    .await
                    .unwrap()
                    .delivery
                    .kind(),
                "unsupported"
            );
            assert_ne!(
                tools
                    .update(caller(&f), &reference, message(true))
                    .await
                    .unwrap()
                    .delivery
                    .kind(),
                "confirmed"
            );
            assert_eq!(writes(&f).len(), 1);
        }
    }
}

#[tokio::test]
async fn card_effects_partial_reaction_and_unknown_transport_fence_new_keys_and_edits() {
    for partial in [false, true] {
        let f = FixtureState::new().await;
        let card = f.card(&message(false)).await;
        let reference = reference(&card);
        let mut tools = state(&f, true);
        let _engine = engine(&f, &mut tools);
        echo(
            &f,
            StatusCode::NO_CONTENT,
            if partial {
                json!([{"me":true,"emoji":{"id":null,"name":"👍"}}])
            } else {
                json!([])
            },
        );
        let memory = Arc::downgrade(&f.rest.memory);
        let prior = f.rest.memory.on_call.lock().unwrap().clone().unwrap();
        *f.rest.memory.on_call.lock().unwrap() = Some(Arc::new(move |call| {
            prior(call);
            if call.method == "PUT" {
                let memory = memory.upgrade().unwrap();
                let mut responses = memory.responses.lock().unwrap();
                responses.pop_back();
                responses.push_back((
                    if partial {
                        StatusCode::FORBIDDEN
                    } else {
                        StatusCode::INTERNAL_SERVER_ERROR
                    },
                    json!({}),
                ));
            }
        }));
        let result = tools
            .set_message_reaction(
                caller(&f),
                &reference,
                "uncertain",
                serde_json::from_value(json!({"reaction":[{"type":"emoji","emoji":"🎉"}]}))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(result.delivery.kind(), "unknown");
        let restarted = state(&f, false);
        assert_eq!(
            restarted
                .delete_message(caller(&f), &reference, "new")
                .await
                .unwrap()
                .delivery
                .kind(),
            "rejected"
        );
        assert_eq!(
            tools
                .update(caller(&f), &reference, message(true))
                .await
                .unwrap()
                .delivery
                .kind(),
            "rejected"
        );
        assert_eq!(writes(&f).len(), if partial { 2 } else { 1 });
        assert!(!f.store.load(&card.card.id).await.unwrap().0.superseded);
    }
}

#[tokio::test]
async fn card_effects_thread_parent_guild_type_and_bot_mismatch_never_write() {
    for case in ["parent_id", "guild_id", "type", "bot"] {
        let f = FixtureState::named_thread(11).await;
        let card = f.card(&message(false)).await;
        let reference = reference(&card);
        let tools = state(&f, true);
        echo(&f, StatusCode::NO_CONTENT, json!([]));
        if case == "bot" {
            *f.rest.memory.bot.lock().unwrap() = json!({"id":"78","bot":true});
        } else {
            let mut metadata = f.thread_metadata();
            metadata[case] = if case == "type" {
                10.into()
            } else {
                "101".into()
            };
            f.rest
                .memory
                .responses
                .lock()
                .unwrap()
                .push_back((StatusCode::OK, metadata));
        }
        assert_eq!(
            tools
                .delete_message(caller(&f), &reference, "changed")
                .await
                .unwrap()
                .delivery
                .kind(),
            "rejected"
        );
        assert!(writes(&f).is_empty());
        assert!(!f.store.load(&card.card.id).await.unwrap().0.superseded);
    }
}
