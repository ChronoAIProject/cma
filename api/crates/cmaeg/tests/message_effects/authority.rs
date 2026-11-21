use super::*;

#[tokio::test]
async fn context_advertises_effects_only_with_matching_native_ingress_evidence() {
    let (app, platform, _) = setup().await;
    let event = app.event("42", "native-capabilities").await;
    for variant in ["current", "legacy", "actor", "conversation"] {
        let mut payload = event.open(&app.state.keys, support::now()).unwrap().payload;
        match variant {
            "legacy" => payload.telegram_admission = None,
            "actor" => payload.telegram_admission.as_mut().unwrap().actor_id = "99".into(),
            "conversation" => {
                payload
                    .telegram_admission
                    .as_mut()
                    .unwrap()
                    .conversation_kind = cmaeg_core::activity::ConversationKind::Channel
            }
            _ => {}
        }
        let reference = cmaeg_core::seal::EventRef::seal(&payload, &app.state.keys).unwrap();
        let result = receipt(
            app.client
                .get(format!(
                    "{}/v1/events/{}/context",
                    app.base,
                    reference.as_str()
                ))
                .bearer_auth(&app.key)
                .send()
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(
            result["capabilities"]["message_effects"]["delete"],
            variant == "current",
            "{variant}"
        );
    }
    assert_eq!(platform.calls.lock().unwrap().len(), 1);
    let mut relay = App::new(vec![]).await;
    relay.state.telegram_effects_issuance_enabled = true;
    relay.restart().await;
    let event = relay.event("42", "relay-capabilities").await;
    let result = receipt(
        relay
            .client
            .get(format!(
                "{}/v1/events/{}/context",
                relay.base,
                event.as_str()
            ))
            .bearer_auth(&relay.key)
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(result["capabilities"]["message_effects"]["delete"], false);
    assert_eq!(result["capabilities"]["message_effects"]["reaction"], false);
}

#[tokio::test]
async fn effects_refuse_missing_duplicate_or_invalid_idempotency_keys_before_io() {
    let (app, platform, reference) = setup().await;
    let url = format!("{}/v1/messages/{}/delete", app.base, reference.as_str());
    assert_eq!(
        app.client
            .post(&url)
            .bearer_auth(&app.key)
            .json(&json!({}))
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    for key in [String::new(), "contains a space".into(), "x".repeat(129)] {
        assert_eq!(
            effect(&app, &reference, "delete", &key, json!({}))
                .send()
                .await
                .unwrap()
                .status(),
            400
        );
    }
    assert_eq!(
        effect(&app, &reference, "delete", "one", json!({}))
            .header("idempotency-key", "two")
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    assert_eq!(
        effect(
            &app,
            &reference,
            "delete",
            "ids",
            json!({"chat_id":"42","message_id":"10"})
        )
        .send()
        .await
        .unwrap()
        .status(),
        400
    );
    assert_eq!(platform.calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn legacy_edit_survives_effect_expiry_without_renewing_effect_authority() {
    let (mut app, platform, reference) = setup().await;
    app.state.telegram_effects_issuance_enabled = false;
    app.restart().await;
    for retained in [false, true] {
        let mut payload = reference.open(&app.state.keys).unwrap().payload;
        if retained {
            let authority = payload.telegram_effect.as_mut().unwrap();
            authority.issued_at = support::now() - 86401;
            authority.expires_at = support::now() - 1;
        } else {
            payload.telegram_effect = None;
        }
        let previous = payload.telegram_effect.clone();
        let reference = MessageRef::seal(&payload, &app.state.keys).unwrap();
        let result = receipt(
            effect(
                &app,
                &reference,
                "update",
                "edit",
                json!({"text":format!("updated {retained}")}),
            )
            .send()
            .await
            .unwrap(),
        )
        .await;
        assert_eq!(result["delivery"]["outcome"], "confirmed");
        let updated = MessageRef::new(result["message_ref"].as_str().unwrap()).unwrap();
        assert_eq!(
            updated
                .open(&app.state.keys)
                .unwrap()
                .payload
                .telegram_effect,
            previous
        );
        if retained {
            assert_eq!(
                effect(&app, &updated, "delete", "expired", json!({}))
                    .send()
                    .await
                    .unwrap()
                    .status(),
                410
            );
        }
    }
    assert_eq!(platform.calls.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn named_consumer_effect_authority_cannot_be_borrowed_or_survive_removal() {
    let (mut app, platform, _) = setup().await;
    let reviewer = routing_fixture::REVIEWER;
    let key = routing_fixture::agent_key(reviewer);
    let mut definition = app.definition.to_blob_json();
    definition["routing"] = json!({"version":1,"default_consumer":{"name":"lead"},"consumers":[routing_fixture::consumer(reviewer,"reviewer","cma_codex")],"default_targets":["default"]});
    definition["secrets"]["consumer_keys"] = json!([[reviewer, key]]);
    app.definition = serde_json::from_value(definition).unwrap();
    app.update().await;
    app.restart().await;
    let event = app
        .event_for(
            "42",
            "reviewer-effect",
            Some(cmaeg_core::ids::ConsumerId::parse(reviewer).unwrap()),
            Secret::new(&key),
        )
        .await;
    let sent = receipt(
        app.client
            .post(format!("{}/v1/events/{}/replies", app.base, event.as_str()))
            .bearer_auth(&key)
            .json(&json!({"text":"reviewer reply"}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    let reference = MessageRef::new(sent["message_ref"].as_str().unwrap()).unwrap();
    assert_eq!(
        effect(&app, &reference, "delete", "borrowed", json!({}))
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    let response = effect_as(&app, &reference, "delete", "owner", json!({}), &key)
        .send()
        .await
        .unwrap();
    assert_eq!(receipt(response).await["delivery"]["outcome"], "confirmed");
    app.definition.routing = None;
    app.definition.secrets.consumer_keys.clear();
    app.update().await;
    assert_eq!(
        effect_as(&app, &reference, "delete", "removed", json!({}), &key)
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(platform.calls.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn reply_to_bot_and_mention_admission_remain_supported_from_sealed_facts() {
    for (groups, mentioned, replied) in [
        ("mention", true, false),
        ("reply_to_bot", false, true),
        ("mention_or_reply_to_bot", false, true),
    ] {
        let (mut app, _, reference) = setup().await;
        if let Source::Telegram { admission, .. } = &mut app.definition.sources[0] {
            *admission = serde_json::from_value(json!({"type":"scoped","senders":{"type":"open"},"chats":{"type":"open"},"groups":groups})).unwrap();
        }
        app.update().await;
        let mut payload = reference.open(&app.state.keys).unwrap().payload;
        let proof = &mut payload.telegram_effect.as_mut().unwrap().admission;
        proof.conversation_kind = cmaeg_core::activity::ConversationKind::Group;
        proof.mentioned = Some(mentioned);
        proof.replied_to_bot = Some(replied);
        let reference = MessageRef::seal(&payload, &app.state.keys).unwrap();
        assert_eq!(
            receipt(
                effect(
                    &app,
                    &reference,
                    "reaction",
                    "supported",
                    json!({"reaction":[]})
                )
                .send()
                .await
                .unwrap()
            )
            .await["delivery"]["outcome"],
            "confirmed",
            "{groups}"
        );
    }
}

#[tokio::test]
async fn mcp_discovery_and_effects_follow_the_same_scoped_contract() {
    let (mut app, platform, reference) = setup().await;
    let rpc = |app: &App, method: &str, params: Value| {
        app.client
            .post(format!("{}/mcp", app.base))
            .bearer_auth(&app.key)
            .json(&json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}))
    };
    let listed = receipt(rpc(&app, "tools/list", json!({})).send().await.unwrap()).await;
    let names: Vec<_> = listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["name"].as_str().unwrap())
        .collect();
    for name in [
        "readMessageCapabilities",
        "deleteMessage",
        "setMessageReaction",
    ] {
        assert!(names.contains(&name));
    }
    let params = json!({"name":"setMessageReaction","arguments":{"message_ref":reference.as_str(),"idempotency_key":"mcp-reaction","request":{"reaction":[{"type":"custom_emoji","custom_emoji_id":"12345"}]}}});
    let response = receipt(rpc(&app, "tools/call", params).send().await.unwrap()).await;
    assert!(response.get("error").is_none(), "{response}");
    assert_eq!(platform.calls.lock().unwrap().len(), 2);
    app.state.telegram_effects_issuance_enabled = false;
    app.restart().await;
    let listed = receipt(rpc(&app, "tools/list", json!({})).send().await.unwrap()).await;
    assert_eq!(listed["result"]["tools"].as_array().unwrap().len(), 4);
    // Disabling issuance preserves readers for already issued references.
    assert_eq!(
        receipt(
            effect(&app, &reference, "delete", "retained", json!({}))
                .send()
                .await
                .unwrap()
        )
        .await["delivery"]["outcome"],
        "confirmed"
    );
}

#[tokio::test]
async fn relay_references_explicitly_refuse_delete_and_reaction_on_every_platform() {
    let (app, platform, reference) = setup().await;
    for name in ["telegram", "lark", "feishu", "whatsapp", "discord"] {
        let mut payload = reference.open(&app.state.keys).unwrap().payload;
        payload.telegram_effect = None;
        payload.route_id = format!("relay-{name}");
        let reference = MessageRef::seal(&payload, &app.state.keys).unwrap();
        let caps = capabilities(&app, &reference).await;
        assert_eq!(caps["delete"], false);
        assert_eq!(caps["reaction"], false);
        for (operation, body) in [("delete", json!({})), ("reaction", json!({"reaction":[]}))] {
            assert_eq!(
                receipt(
                    effect(&app, &reference, operation, name, body)
                        .send()
                        .await
                        .unwrap()
                )
                .await["delivery"]["outcome"],
                "unsupported"
            );
        }
    }
    assert_eq!(platform.calls.lock().unwrap().len(), 1);
}
