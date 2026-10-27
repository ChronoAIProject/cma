//! Upload references use the same current authority, receipts and mutation fence.
use super::*;

fn upload() -> cmaeg_core::message::OutboundMessage {
    serde_json::from_value(json!({"native_attachments":[{
        "kind":"file", "filename":"report.txt",
        "source":{"type":"base64","data":"AQIDBA=="}
    }]}))
    .unwrap()
}

async fn app() -> App {
    let mut app = App::new().await;
    app.media_effects = true;
    app.state.native_discord_edits_enabled = false;
    app.restart().await;
    app
}

async fn reference(app: &App) -> MessageRef {
    let sent = app.send_message(upload()).await;
    assert_eq!(sent.delivery.kind(), "confirmed");
    let reference = sent.message_ref.unwrap();
    assert!(
        reference
            .open(&app.state.keys)
            .unwrap()
            .payload
            .discord_message
            .unwrap()
            .effects_only
    );
    reference
}

#[tokio::test]
async fn upload_effects_survive_gate_off_and_restart_without_editing_or_duplicate_writes() {
    let mut app = app().await;
    let reference = reference(&app).await;
    app.media_effects = false;
    app.state.discord_effects_issuance_enabled = false;
    app.state.native_discord_edits_enabled = true;
    app.restart().await;
    assert_eq!(app.caps(&reference).await["delete"], true);
    assert_eq!(app.caps(&reference).await["reaction"], true);
    assert_eq!(
        value(
            app.effect(&reference, "update", "edit", json!({"text":"updated"}))
                .send()
                .await
                .unwrap()
        )
        .await["delivery"]["outcome"],
        "unsupported"
    );
    assert_eq!(app.platform.memory.lock().unwrap().writes.len(), 1);
    // Attachment changes do not turn the exact message into a different target.
    // The retained scope still has no attachment or content update authority.
    app.platform.memory.lock().unwrap().message["attachments"] = json!([]);
    app.platform.memory.lock().unwrap().message["reactions"] = json!([
        {"me":true,"emoji":{"name":"👀","id":null}},
        {"me":false,"emoji":{"name":"🎉","id":null}}
    ]);
    let reacted = value(
        app.effect(
            &reference,
            "reaction",
            "react",
            json!({"reaction":[{"type":"emoji","emoji":"👍"}]}),
        )
        .send()
        .await
        .unwrap(),
    )
    .await;
    assert_eq!(reacted["delivery"]["outcome"], "confirmed");
    app.restart().await;
    assert_eq!(
        value(
            app.effect(
                &reference,
                "reaction",
                "react",
                json!({"reaction":[{"type":"emoji","emoji":"👍"}]})
            )
            .send()
            .await
            .unwrap()
        )
        .await,
        reacted
    );
    app.platform.memory.lock().unwrap().message["reactions"] = json!([
        {"me":true,"emoji":{"name":"👍","id":null}},
        {"me":false,"emoji":{"name":"🎉","id":null}}
    ]);
    assert_eq!(
        value(
            app.effect(&reference, "reaction", "clear", json!({"reaction":[]}))
                .send()
                .await
                .unwrap()
        )
        .await["delivery"]["outcome"],
        "confirmed"
    );
    let deleted = value(
        app.effect(&reference, "delete", "delete", json!({}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(deleted["delivery"]["outcome"], "confirmed");
    app.restart().await;
    assert_eq!(
        value(
            app.effect(&reference, "delete", "delete", json!({}))
                .send()
                .await
                .unwrap()
        )
        .await,
        deleted
    );
    let memory = app.platform.memory.lock().unwrap();
    assert_eq!(memory.writes.len(), 5);
    assert_eq!(
        memory.writes[0],
        (Method::POST, "/api/v10/channels/99/messages".into())
    );
    for (method, path) in &memory.writes[1..4] {
        assert!(matches!(*method, Method::DELETE | Method::PUT));
        assert!(path.starts_with("/api/v10/channels/99/messages/800/reactions/"));
        assert!(path.ends_with("/@me"));
        assert!(
            !path.contains("%F0%9F%8E%89"),
            "another user's reaction must remain"
        );
    }
    assert_eq!(
        memory.writes[4],
        (Method::DELETE, "/api/v10/channels/99/messages/800".into())
    );
}

#[tokio::test]
async fn upload_effect_context_does_not_require_advertising_text_edits() {
    let app = app().await;
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
    assert!(!view.capabilities.edit);
    assert!(view.capabilities.message_effects.unwrap().delete);
    assert!(app.send().await.message_ref.is_none());
    reference(&app).await;
}

#[tokio::test]
async fn upload_authority_rechecks_current_owner_source_provider_and_key() {
    for changed in ["owner", "source", "provider", "key"] {
        let mut app = app().await;
        let reference = reference(&app).await;
        app.caps(&reference).await;
        match changed {
            "owner" => app.definition.owner.subject = "new-owner".into(),
            "source" => app.definition.sources.clear(),
            "provider" => definition_fixture::replace_default_binding(
                &mut app.definition,
                "bnd_changed".parse().unwrap(),
            ),
            "key" => app.definition.secrets.agent_key_hash = format!("sha256:{}", "1".repeat(64)),
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
async fn named_upload_reference_requires_its_own_consumer_and_live_binding() {
    let mut app = app().await;
    let id = routing_fixture::REVIEWER;
    let key = routing_fixture::agent_key(id);
    let mut definition = app.definition.to_blob_json();
    definition["routing"] = json!({"version":1,"default_consumer":{"name":"lead"},"consumers":[routing_fixture::consumer(id,"reviewer","cma_codex")],"default_targets":["default"]});
    definition["secrets"]["consumer_keys"] = json!([[id, key]]);
    app.definition = serde_json::from_value(definition).unwrap();
    app.update().await;
    app.event.consumer = Some(cmaeg_core::ids::ConsumerId::parse(id).unwrap());
    let reference = reference(&app).await;
    assert_eq!(
        app.effect(&reference, "delete", "borrowed", json!({}))
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    app.key = key;
    assert_eq!(app.caps(&reference).await["delete"], true);
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
    assert_eq!(app.platform.memory.lock().unwrap().writes.len(), 1);
}

#[tokio::test]
async fn uncertain_upload_effect_keeps_the_exact_message_fenced_after_restart() {
    let mut app = app().await;
    let reference = reference(&app).await;
    app.platform.memory.lock().unwrap().reject_write = Some((2, StatusCode::BAD_GATEWAY));
    let uncertain = value(
        app.effect(&reference, "delete", "uncertain", json!({}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(uncertain["delivery"]["outcome"], "unknown");
    app.restart().await;
    assert_eq!(
        value(
            app.effect(&reference, "delete", "uncertain", json!({}))
                .send()
                .await
                .unwrap()
        )
        .await,
        uncertain
    );
    assert_eq!(
        value(
            app.effect(&reference, "reaction", "new-key", json!({"reaction":[]}))
                .send()
                .await
                .unwrap()
        )
        .await["delivery"]["reason"],
        "native_message_effect_busy"
    );
    assert_eq!(app.platform.memory.lock().unwrap().writes.len(), 2);
}

#[tokio::test]
async fn expired_upload_authority_refuses_effects_and_never_becomes_edit_authority() {
    let app = app().await;
    let reference = reference(&app).await;
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
    assert_eq!(
        value(
            app.effect(&expired, "update", "edit", json!({"text":"updated"}))
                .send()
                .await
                .unwrap()
        )
        .await["delivery"]["outcome"],
        "unsupported"
    );
    assert_eq!(app.platform.memory.lock().unwrap().writes.len(), 1);
}
