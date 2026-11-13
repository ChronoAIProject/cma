use super::*;

#[tokio::test]
async fn actual_native_issuance_and_public_effects_survive_restart_without_duplicate_writes() {
    let mut app = App::new().await;
    let reference = app.reference().await;
    assert_eq!(app.caps(&reference).await["delete"], true);
    assert_eq!(app.caps(&reference).await["reaction"], true);
    for (op, key, body) in [
        (
            "reaction",
            "react",
            json!({"reaction":[{"type":"emoji","emoji":"👍"}]}),
        ),
        ("reaction", "clear", json!({"reaction":[]})),
        ("delete", "delete", json!({})),
    ] {
        if op == "reaction" && key == "clear" {
            app.platform.memory.lock().unwrap().message["reactions"] =
                json!([{"me":true,"emoji":{"name":"👍","id":null}}]);
        }
        let first = value(
            app.effect(&reference, op, key, body.clone())
                .send()
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(first["delivery"]["outcome"], "confirmed");
        let before = app.platform.memory.lock().unwrap().writes.len();
        app.restart().await;
        assert_eq!(
            value(app.effect(&reference, op, key, body).send().await.unwrap()).await,
            first
        );
        assert_eq!(app.platform.memory.lock().unwrap().writes.len(), before);
    }
    let memory = app.platform.memory.lock().unwrap();
    assert_eq!(memory.writes.len(), 4);
    assert_eq!(memory.writes[1].0, Method::PUT);
    assert!(memory.writes[1].1.ends_with("/@me"));
    assert_eq!(
        memory.writes[3],
        (Method::DELETE, "/api/v10/channels/99/messages/800".into())
    );
}

#[tokio::test]
async fn uncertainty_and_rejected_or_rate_limited_effects_have_durable_receipts() {
    for (status, expected) in [
        (StatusCode::BAD_GATEWAY, "unknown"),
        (StatusCode::FORBIDDEN, "rejected"),
        (StatusCode::TOO_MANY_REQUESTS, "rate_limited"),
    ] {
        let mut app = App::new().await;
        let reference = app.reference().await;
        app.platform.memory.lock().unwrap().reject_write = Some((2, status));
        let first = value(
            app.effect(&reference, "delete", "one", json!({}))
                .send()
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(first["delivery"]["outcome"], expected);
        app.restart().await;
        assert_eq!(
            value(
                app.effect(&reference, "delete", "one", json!({}))
                    .send()
                    .await
                    .unwrap()
            )
            .await,
            first
        );
        assert_eq!(app.platform.memory.lock().unwrap().writes.len(), 2);
    }
}

#[tokio::test]
async fn concurrent_effects_share_one_claim_and_conflicting_action_is_rejected() {
    let app = App::new().await;
    let reference = app.reference().await;
    let (a, b) = tokio::join!(
        app.effect(&reference, "delete", "same", json!({})).send(),
        app.effect(&reference, "delete", "same", json!({})).send()
    );
    let a = value(a.unwrap()).await;
    let b = value(b.unwrap()).await;
    assert!(a["delivery"]["outcome"] == "confirmed" || b["delivery"]["outcome"] == "confirmed");
    assert_eq!(app.platform.memory.lock().unwrap().writes.len(), 2);
    assert_eq!(
        app.effect(&reference, "reaction", "same", json!({"reaction":[]}))
            .send()
            .await
            .unwrap()
            .status(),
        409
    );
    assert_eq!(app.platform.memory.lock().unwrap().writes.len(), 2);
}

#[tokio::test]
async fn cached_authority_cannot_survive_source_binding_owner_or_key_changes() {
    for changed in [
        "source",
        "policy",
        "provider",
        "owner",
        "key",
        "token_removed",
    ] {
        let mut app = App::new().await;
        let reference = app.reference().await;
        app.caps(&reference).await;
        match changed {
            "source" => app.definition.sources.clear(),
            "policy" => {
                if let Source::Discord { admission, .. } = &mut app.definition.sources[0] {
                    *admission = AdmissionPolicy::Allowlist {
                        sender_ids: vec!["90".into()],
                    };
                }
            }
            "provider" => definition_fixture::replace_default_binding(
                &mut app.definition,
                "bnd_changed".parse().unwrap(),
            ),
            "owner" => app.definition.owner.subject = "another-owner".into(),
            "key" => app.definition.secrets.agent_key_hash = format!("sha256:{}", "1".repeat(64)),
            "token_removed" => app.definition.secrets.discord_bot_token = None,
            _ => unreachable!(),
        }
        app.update().await;
        assert!(
            !app.effect(&reference, "delete", "revoked", json!({}))
                .send()
                .await
                .unwrap()
                .status()
                .is_success(),
            "{changed}"
        );
        assert_eq!(app.platform.memory.lock().unwrap().writes.len(), 1);
    }
}

#[tokio::test]
async fn effects_gate_does_not_revoke_retained_references_or_renew_expiry_on_edit() {
    let mut app = App::new().await;
    let reference = app.reference().await;
    app.state.discord_effects_issuance_enabled = false;
    app.restart().await;
    let edit_only = app.reference().await;
    assert_eq!(app.caps(&edit_only).await["delete"], false);
    assert_eq!(app.caps(&reference).await["delete"], true);
    let mut payload = reference.open(&app.state.keys).unwrap().payload;
    let window = payload
        .discord_message
        .as_mut()
        .unwrap()
        .effects
        .as_mut()
        .unwrap();
    window.issued_at = now() - 86401;
    window.expires_at = now() - 1;
    let expired = MessageRef::seal(&payload, &app.state.keys).unwrap();
    assert_eq!(
        app.effect(&expired, "delete", "expired", json!({}))
            .send()
            .await
            .unwrap()
            .status(),
        410
    );
    let edited = value(
        app.effect(&expired, "update", "edit", json!({"text":"updated"}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(edited["delivery"]["outcome"], "confirmed");
    let edited = MessageRef::new(edited["message_ref"].as_str().unwrap())
        .unwrap()
        .open(&app.state.keys)
        .unwrap()
        .payload;
    assert_eq!(
        edited.discord_message.unwrap().effects,
        payload.discord_message.unwrap().effects
    );
}

#[tokio::test]
async fn context_capabilities_and_new_references_require_both_issuance_flags() {
    for (edits, effects) in [(false, false), (true, false), (false, true), (true, true)] {
        let mut app = App::new().await;
        app.state.native_discord_edits_enabled = edits;
        app.state.discord_effects_issuance_enabled = effects && edits;
        app.restart().await;
        let event = EventRef::seal(&app.event, &app.state.keys).unwrap();
        let view = app
            .state
            .verify_context(
                AgentCaller {
                    key_hash: Sha256::digest(app.key.as_bytes()).into(),
                },
                &event,
                app.activity.clone(),
            )
            .await
            .unwrap();
        assert_eq!(
            view.capabilities
                .message_effects
                .as_ref()
                .is_some_and(|caps| caps.delete),
            effects && edits
        );
        let sent = app.send().await;
        assert_eq!(sent.message_ref.is_some(), edits);
        if let Some(reference) = sent.message_ref {
            assert_eq!(
                reference
                    .open(&app.state.keys)
                    .unwrap()
                    .payload
                    .discord_message
                    .unwrap()
                    .effects
                    .is_some(),
                effects
            );
        }
    }
}

#[tokio::test]
async fn loss_of_final_receipt_cannot_repeat_an_already_applied_discord_effect() {
    use std::sync::atomic::Ordering;
    let mut app = App::new().await;
    let reference = app.reference().await;
    app.platform.memory.lock().unwrap().fail_receipt = Some(app.backend.fail_put.clone());
    let first = value(
        app.effect(&reference, "delete", "uncertain", json!({}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(first["delivery"]["outcome"], "unknown");
    app.platform.memory.lock().unwrap().fail_receipt = None;
    app.backend.fail_put.store(false, Ordering::SeqCst);
    app.restart().await;
    let replay = value(
        app.effect(&reference, "delete", "uncertain", json!({}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(replay["delivery"]["outcome"], "unknown");
    assert_eq!(app.platform.memory.lock().unwrap().writes.len(), 2);
}

#[tokio::test]
async fn named_consumer_reference_requires_its_own_live_key_and_binding() {
    let mut app = App::new().await;
    let id = routing_fixture::REVIEWER;
    let key = routing_fixture::agent_key(id);
    let mut definition = app.definition.to_blob_json();
    definition["routing"] = json!({"version":1,"default_consumer":{"name":"lead"},"consumers":[routing_fixture::consumer(id,"reviewer","cma_codex")],"default_targets":["default"]});
    definition["secrets"]["consumer_keys"] = json!([[id, key]]);
    app.definition = serde_json::from_value(definition).unwrap();
    app.update().await;
    app.event.consumer = Some(cmaeg_core::ids::ConsumerId::parse(id).unwrap());
    let reference = app.reference().await;
    assert_eq!(
        app.effect(&reference, "delete", "borrowed", json!({}))
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    app.key = key;
    assert_eq!(
        value(
            app.effect(&reference, "delete", "owner", json!({}))
                .send()
                .await
                .unwrap()
        )
        .await["delivery"]["outcome"],
        "confirmed"
    );
    app.definition.routing = None;
    app.definition.secrets.consumer_keys.clear();
    app.update().await;
    assert_eq!(
        app.effect(&reference, "delete", "removed", json!({}))
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(app.platform.memory.lock().unwrap().writes.len(), 2);
}

#[tokio::test]
async fn mcp_lists_discord_effects_and_shares_the_rest_idempotency_receipt() {
    let app = App::new().await;
    let reference = app.reference().await;
    let call = |method: &str, params: Value| {
        reqwest::Client::new()
            .post(format!("{}/mcp", app.base))
            .bearer_auth(&app.key)
            .header("accept", "application/json, text/event-stream")
            .json(&json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}))
    };
    let listed = value(call("tools/list", json!({})).send().await.unwrap()).await;
    assert!(listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tool| tool["name"] == "deleteMessage"));
    let result = value(call("tools/call",json!({"name":"deleteMessage","arguments":{"message_ref":reference.as_str(),"idempotency_key":"across-surfaces"}})).send().await.unwrap()).await;
    assert!(result.get("error").is_none(), "{result}");
    assert_ne!(result["result"]["isError"], true, "{result}");
    let replay = value(
        app.effect(&reference, "delete", "across-surfaces", json!({}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(replay["delivery"]["outcome"], "confirmed");
    assert_eq!(app.platform.memory.lock().unwrap().writes.len(), 2);
}

#[tokio::test]
async fn different_keys_on_separate_http_instances_cannot_replace_one_reaction_concurrently() {
    let app = App::new().await;
    let reference = app.reference().await;
    let entered = Arc::new(tokio::sync::Semaphore::new(0));
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    app.platform.memory.lock().unwrap().message_gate = Some((entered.clone(), release.clone()));
    let first = app.effect(
        &reference,
        "reaction",
        "first",
        json!({"reaction":[{"type":"emoji","emoji":"👍"}]}),
    );
    let first = tokio::spawn(async move { first.send().await.unwrap() });
    tokio::time::timeout(Duration::from_secs(2), entered.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    // A separate HTTP composition has its own client/capacity. Only the record
    // service can serialize these different idempotency keys.
    let mut second_state = app.state.clone();
    second_state.discord_effects = Some(Arc::new(discord::DiscordEffectsClient::new(
        app.platform.api.clone(),
    )));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!(
        "http://{}/v1/messages/{}/reaction",
        listener.local_addr().unwrap(),
        reference.as_str()
    );
    let router = http::tools::router(second_state);
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let second = tokio::time::timeout(
        Duration::from_secs(2),
        reqwest::Client::new()
            .post(url)
            .bearer_auth(&app.key)
            .header("idempotency-key", "second")
            .json(&json!({"reaction":[{"type":"emoji","emoji":"🎉"}]}))
            .send(),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        value(second).await["delivery"],
        json!({"outcome":"rejected","reason":"native_message_effect_busy"})
    );
    release.add_permits(1);
    assert_eq!(
        value(first.await.unwrap()).await["delivery"]["outcome"],
        "confirmed"
    );
    server.abort();
    assert_eq!(app.platform.memory.lock().unwrap().writes.len(), 2);
    assert_eq!(
        entered.available_permits(),
        0,
        "the conflicting request cannot read or mutate the platform"
    );
    app.platform.memory.lock().unwrap().message_gate = None;
    assert_eq!(
        value(
            app.effect(
                &reference,
                "reaction",
                "after-completion",
                json!({"reaction":[]})
            )
            .send()
            .await
            .unwrap()
        )
        .await["delivery"]["outcome"],
        "confirmed"
    );
}

#[tokio::test]
async fn new_keys_cannot_bypass_an_ambiguous_message_fence_after_restart() {
    let mut app = App::new().await;
    let reference = app.reference().await;
    app.platform.memory.lock().unwrap().reject_write = Some((2, StatusCode::BAD_GATEWAY));
    assert_eq!(
        value(
            app.effect(&reference, "delete", "uncertain", json!({}))
                .send()
                .await
                .unwrap()
        )
        .await["delivery"]["outcome"],
        "unknown"
    );
    app.restart().await;
    assert_eq!(
        value(
            app.effect(
                &reference,
                "reaction",
                "new-key",
                json!({"reaction":[{"type":"emoji","emoji":"👍"}]})
            )
            .send()
            .await
            .unwrap()
        )
        .await["delivery"],
        json!({"outcome":"rejected","reason":"native_message_effect_busy"})
    );
    assert_eq!(app.platform.memory.lock().unwrap().writes.len(), 2);
}
