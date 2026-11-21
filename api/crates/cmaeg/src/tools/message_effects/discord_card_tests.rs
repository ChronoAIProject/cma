use super::discord_card_fixture::*;
use crate::discord::components_fixture::{message, FixtureState};
use axum::http::StatusCode;
use serde_json::json;

#[tokio::test]
async fn card_effects_adopt_once_preserve_authority_and_execute_gate_off_after_restart() {
    for named_thread in [false, true] {
        let f = if named_thread {
            FixtureState::named_thread(11).await
        } else {
            FixtureState::new().await
        };
        let card = f.card(&message(false)).await;
        let reference = reference(&card);
        let before = serde_json::to_value(&card.card).unwrap();
        assert!(before.get("discord_effect").is_none());
        let disabled = state(&f, false);
        assert!(
            !disabled
                .message_effect_capabilities(caller(&f), &reference)
                .await
                .unwrap()
                .delete
        );
        assert_eq!(
            disabled
                .delete_message(caller(&f), &reference, "off")
                .await
                .unwrap()
                .delivery
                .kind(),
            "unsupported"
        );
        assert!(writes(&f).is_empty());
        let enabled = state(&f, true);
        let capabilities = enabled
            .message_effect_capabilities(caller(&f), &reference)
            .await
            .unwrap();
        assert!(capabilities.delete && capabilities.reaction);
        assert_eq!(capabilities.expires_at, Some(card.card.expires_at));
        let after = serde_json::to_value(f.store.load(&card.card.id).await.unwrap().0).unwrap();
        let mut expected = before.clone();
        expected["discord_effect"] = json!({});
        assert_eq!(after, expected);
        echo(&f, StatusCode::NO_CONTENT, json!([]));
        let receipt = disabled
            .set_message_reaction(
                caller(&f),
                &reference,
                "retained",
                serde_json::from_value(json!({"reaction":[{"type":"emoji","emoji":"👍"}]}))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(receipt.delivery.kind(), "confirmed");
        assert!(!f.store.load(&card.card.id).await.unwrap().0.superseded);
        let restarted = state(&f, false);
        assert_eq!(
            restarted
                .set_message_reaction(
                    caller(&f),
                    &reference,
                    "retained",
                    serde_json::from_value(json!({"reaction":[{"type":"emoji","emoji":"👍"}]}))
                        .unwrap()
                )
                .await
                .unwrap(),
            receipt
        );
        assert_eq!(writes(&f).len(), 1);
    }
}

#[tokio::test]
async fn card_effects_rest_mcp_share_exact_receipts_conflicts_and_deleted_replay() {
    let f = FixtureState::named().await;
    let card = f.card(&message(false)).await;
    let reference = reference(&card);
    let state = state(&f, true);
    echo(&f, StatusCode::NO_CONTENT, json!([]));
    let path = format!("/v1/messages/{}/reaction", reference.as_str());
    let body = json!({"reaction":[{"type":"emoji","emoji":"🎉"}]});
    let (status, first) = http_call(&f, &state, &path, body.clone(), Some("same")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(first["delivery"]["outcome"], "confirmed");
    let (status, replay) = http_call(&f, &state, &path, body, Some("same")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replay, first);
    let (status, conflict) = http_call(
        &f,
        &state,
        &format!("/v1/messages/{}/delete", reference.as_str()),
        json!({}),
        Some("same"),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(conflict
        .to_string()
        .contains("message_effect_idempotency_conflict"));
    let rpc = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"deleteMessage","arguments":{"message_ref":reference,"idempotency_key":"delete"}}});
    let (status, deleted) = http_call(&f, &state, "/mcp", rpc.clone(), None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(deleted.to_string().contains("confirmed"), "{deleted}");
    assert!(f.store.load(&card.card.id).await.unwrap().0.superseded);
    let (_, again) = http_call(&f, &state, "/mcp", rpc, None).await;
    assert_eq!(again, deleted);
    assert_eq!(
        state
            .delete_message(caller(&f), &reference, "new")
            .await
            .unwrap()
            .delivery
            .kind(),
        "unsupported"
    );
    assert_eq!(writes(&f).len(), 2);
}

#[tokio::test]
async fn card_effects_old_revision_replays_but_successor_has_its_own_scope_and_inherits_adoption() {
    let f = FixtureState::new().await;
    let card = f.card(&message(false)).await;
    let old = reference(&card);
    let mut tools = state(&f, true);
    let _engine = engine(&f, &mut tools);
    echo(&f, StatusCode::NO_CONTENT, json!([]));
    let reaction = serde_json::from_value(json!({"reaction":[]})).unwrap();
    let first = tools
        .set_message_reaction(caller(&f), &old, "same", reaction)
        .await
        .unwrap();
    let edited = tools.update(caller(&f), &old, message(true)).await.unwrap();
    assert_eq!(edited.delivery.kind(), "confirmed");
    let next = edited.message_ref.unwrap();
    let mut gate_off = tools.clone();
    gate_off.discord_effects = state(&f, false).discord_effects;
    assert_eq!(
        gate_off
            .set_message_reaction(
                caller(&f),
                &old,
                "same",
                serde_json::from_value(json!({"reaction":[]})).unwrap()
            )
            .await
            .unwrap(),
        first
    );
    assert_eq!(
        gate_off
            .delete_message(caller(&f), &old, "new")
            .await
            .unwrap()
            .delivery
            .kind(),
        "unsupported"
    );
    assert_eq!(
        gate_off
            .delete_message(caller(&f), &next, "same")
            .await
            .unwrap()
            .delivery
            .kind(),
        "confirmed"
    );
    let payload = next.open(&f.keys()).unwrap().payload;
    let successor = f
        .store
        .load(payload.interaction_record_id.as_ref().unwrap())
        .await
        .unwrap()
        .0;
    assert_eq!(successor.expires_at, card.card.expires_at);
    assert_eq!(successor.revision, 2);
    assert!(successor.discord_effect.is_some());
    assert_eq!(writes(&f).len(), 2);
}

#[tokio::test]
async fn card_effects_only_bot_reactions_are_removed_and_is_big_is_refused() {
    let f = FixtureState::group().await;
    let card = f.card(&message(false)).await;
    let reference = reference(&card);
    let tools = state(&f, true);
    echo(
        &f,
        StatusCode::NO_CONTENT,
        json!([{"me":true,"emoji":{"id":null,"name":"👍"}},{"me":false,"emoji":{"id":null,"name":"👀"}}]),
    );
    for (key, request) in [
        (
            "replace",
            json!({"reaction":[{"type":"emoji","emoji":"🎉"}]}),
        ),
        ("clear", json!({"reaction":[]})),
    ] {
        assert_eq!(
            tools
                .set_message_reaction(
                    caller(&f),
                    &reference,
                    key,
                    serde_json::from_value(request).unwrap()
                )
                .await
                .unwrap()
                .delivery
                .kind(),
            "confirmed"
        );
    }
    let calls = writes(&f);
    assert_eq!(calls.len(), 3);
    assert!(calls.iter().all(|(_, path)| path.ends_with("/@me")));
    assert!(!calls.iter().any(|(_, path)| path.contains("%F0%9F%91%80")));
    let result = tools
        .set_message_reaction(
            caller(&f),
            &reference,
            "big",
            serde_json::from_value(
                json!({"reaction":[{"type":"emoji","emoji":"👍"}],"is_big":true}),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(result.delivery.kind(), "rejected");
    assert_eq!(writes(&f), calls);
}

#[tokio::test]
async fn card_effects_reject_forged_reference_fields_and_advertise_existing_tools() {
    let f = FixtureState::named().await;
    let card = f.card(&message(false)).await;
    let reference = reference(&card);
    let tools = state(&f, true);
    let payload = reference.open(&f.keys()).unwrap().payload;
    for field in ["event_id", "route_id", "platform_message_id", "consumer"] {
        let mut forged = serde_json::to_value(&payload).unwrap();
        forged[field] = match field {
            "event_id" => "discord:77:701".into(),
            "route_id" => "discord:77:100".into(),
            "platform_message_id" => "801".into(),
            _ => serde_json::Value::Null,
        };
        let forged =
            cmaeg_core::ids::MessageRef::seal(&serde_json::from_value(forged).unwrap(), &f.keys())
                .unwrap();
        assert!(
            tools
                .delete_message(caller(&f), &forged, "forged")
                .await
                .is_err(),
            "{field}"
        );
    }
    assert!(writes(&f).is_empty());
    let (_, listed) = http_call(
        &f,
        &tools,
        "/mcp",
        json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
        None,
    )
    .await;
    assert!(listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tool| tool["name"] == "deleteMessage"));
}
