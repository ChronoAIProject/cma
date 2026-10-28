//! Decoration evolution, durable closure and exact replay use real local boundaries.
use super::*;
use crate::native_media::model::RevisionStatus;

fn digest(app: &App, reference: &MessageRef) -> String {
    reference
        .open(&app.state.keys)
        .unwrap()
        .payload
        .native_media_edit
        .unwrap()
        .original_sha256
}
fn store(app: &App) -> Store {
    Store::new(app.state.resolver.record_client(), app.state.keys.clone())
}
async fn decorated_app() -> (App, MessageRef) {
    let (app, reference) = media_app().await;
    let store = store(&app);
    let (mut original, _) = store.load(&digest(&app, &reference)).await.unwrap();
    {
        let mut memory = app.platform.memory.lock().unwrap();
        memory.message["embeds"] =
            json!([{"type":"link","url":"https://example.com","title":"Original"}]);
        memory.message["flags"] = 4096.into();
        memory.message["attachments"][0]["flags"] = 8.into();
        memory.message["attachments"][0]["filename"] = "SPOILER_report.bin".into();
        original.snapshot = crate::discord::rest::media_edit::snapshot(
            &memory.message,
            "77",
            "99",
            "800",
            NativeMediaKind::File,
        )
        .unwrap();
    }
    let digest = store.issue(&original).await.unwrap();
    let (_, current) = store.load(&digest).await.unwrap();
    let reference = authority::reference(&original, &current, &app.state.keys, None).unwrap();
    (app, reference)
}
async fn entered(semaphore: &tokio::sync::Semaphore) {
    tokio::time::timeout(Duration::from_secs(3), semaphore.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
}
async fn fence_available(app: &App) -> bool {
    let serial =
        mutation::Store::discord(app.state.resolver.record_client(), app.state.keys.clone());
    let held = serial
        .acquire(
            mutation::destination("77", "99", "800"),
            &app.definition.owner.subject,
            now() + 86_400,
        )
        .await
        .unwrap();
    if let Some(held) = held {
        serial.release(held).await.unwrap();
        true
    } else {
        false
    }
}
#[tokio::test]
async fn native_media_content_regenerates_preview_and_preserves_spoiler_without_echoing_embeds() {
    let (app, reference) = decorated_app().await;
    app.platform.memory.lock().unwrap().media_after = Some(
        json!({"embeds":[{"type":"article","title":"New preview","url":"https://new.example"}]}),
    );
    let result = edit(
        &app,
        &reference,
        "content",
        json!({"text":"https://new.example"}),
    )
    .await;
    assert_eq!(result["delivery"]["outcome"], "confirmed");
    let updated = next(&result);
    assert!(app.caps(&updated).await.get("media_edit").is_some());
    {
        let memory = app.platform.memory.lock().unwrap();
        let payload: Value = serde_json::from_slice(&memory.bodies[0].1).unwrap();
        assert!(payload.get("attachments").is_none());
        assert!(payload.get("embeds").is_none());
        assert!(payload.get("flags").is_none());
        assert_eq!(memory.message["attachments"][0]["flags"], 8);
    }
    app.platform.memory.lock().unwrap().media_after = None;
    let result = edit(&app, &updated, "alt", json!({"caption":"new alt"})).await;
    assert!(result.get("message_ref").is_some());
    let replaced = edit(&app,&next(&result),"replace",json!({"replacement":{"kind":"file","source":{"type":"base64","data":"AQIDBA=="},"filename":"new.bin","mime_type":"application/octet-stream"}})).await;
    assert_eq!(replaced["delivery"]["outcome"], "confirmed");
    let memory = app.platform.memory.lock().unwrap();
    assert_eq!(memory.message["attachments"][0]["flags"], 8);
    assert_eq!(memory.message["attachments"][0]["description"], "new alt");
    let wire = String::from_utf8_lossy(&memory.bodies[2].1);
    assert!(wire.contains("\"is_spoiler\":true"));
    assert!(!wire.contains("\"embeds\""));
}
#[tokio::test]
async fn native_media_optional_preview_failure_is_confirmed_retired_and_exactly_replayed() {
    for changed_alt_only in [false, true] {
        let (mut app, reference) = decorated_app().await;
        let embed = if changed_alt_only {
            json!({"type":"link","title":"Changed preview"})
        } else {
            json!({"type":"link","title":"x".repeat(257)})
        };
        app.platform.memory.lock().unwrap().media_after = Some(json!({"embeds":[embed]}));
        let patch = if changed_alt_only {
            json!({"caption":"new"})
        } else {
            json!({"text":"new"})
        };
        let receipt = edit(&app, &reference, "optional", patch.clone()).await;
        assert_eq!(receipt["delivery"]["outcome"], "confirmed");
        assert!(receipt["message_ref"].is_null());
        let (_, current) = store(&app).load(&digest(&app, &reference)).await.unwrap();
        assert!(
            matches!(current.status,Some(RevisionStatus::Retired { ref reason,.. }) if reason == "native_media_optional_presentation_unavailable")
        );
        assert!(app.caps(&reference).await.get("media_edit").is_none());
        assert!(fence_available(&app).await);
        app.restart().await;
        assert_eq!(edit(&app, &reference, "optional", patch).await, receipt);
        assert_eq!(
            edit(&app, &reference, "new-key", json!({"caption":"later"})).await["delivery"]
                ["outcome"],
            "rejected"
        );
        assert_eq!(app.platform.memory.lock().unwrap().writes.len(), 1);
    }
}
#[tokio::test]
async fn native_media_optional_state_storage_failure_keeps_confirmed_receipt_and_fence() {
    for retire in [false, true] {
        let (mut app, reference) = decorated_app().await;
        if retire {
            app.platform.memory.lock().unwrap().media_after =
                Some(json!({"embeds":[{"type":"link","title":"x".repeat(257)}]}));
        }
        app.platform.memory.lock().unwrap().fail_next_put = Some(app.backend.put_failures.clone());
        let receipt = edit(&app, &reference, "optional-store", json!({"text":"new"})).await;
        assert_eq!(receipt["delivery"]["outcome"], "confirmed");
        assert!(receipt["message_ref"].is_null());
        let (_, current) = store(&app).load(&digest(&app, &reference)).await.unwrap();
        assert!(matches!(
            current.status,
            Some(RevisionStatus::Attempting { .. })
        ));
        assert!(app.caps(&reference).await.get("media_edit").is_none());
        assert!(!fence_available(&app).await);
        app.restart().await;
        assert_eq!(
            edit(&app, &reference, "optional-store", json!({"text":"new"})).await,
            receipt
        );
        assert_eq!(
            edit(&app, &reference, "different", json!({"text":"other"})).await["delivery"]
                ["outcome"],
            "unknown"
        );
        assert_eq!(app.platform.memory.lock().unwrap().writes.len(), 1);
    }
}
#[tokio::test]
async fn native_media_external_change_permanently_retires_even_when_provider_reverts() {
    for change in [
        "preview",
        "title",
        "spoiler",
        "flags",
        "content",
        "reply",
        "message_id",
        "channel",
    ] {
        let (mut app, reference) = decorated_app().await;
        let saved = app.platform.memory.lock().unwrap().message.clone();
        let saved_channel = app.platform.memory.lock().unwrap().guild.clone();
        {
            let mut memory = app.platform.memory.lock().unwrap();
            match change {
                "preview" => memory.message["embeds"][0]["title"] = "External".into(),
                "title" => memory.message["attachments"][0]["title"] = "External".into(),
                "spoiler" => memory.message["attachments"][0]["flags"] = 0.into(),
                "flags" => memory.message["flags"] = 0.into(),
                "content" => memory.message["content"] = "External".into(),
                "message_id" => memory.message["id"] = "801".into(),
                "channel" => memory.guild["id"] = "100".into(),
                _ => {
                    memory.message["message_reference"] =
                        json!({"message_id":"701","channel_id":"99"})
                }
            }
        }
        assert_eq!(
            edit(&app, &reference, "changed", json!({"caption":"new"})).await["delivery"]
                ["outcome"],
            "rejected",
            "{change}"
        );
        app.platform.memory.lock().unwrap().message = saved;
        app.platform.memory.lock().unwrap().guild = saved_channel;
        app.restart().await;
        assert!(app.caps(&reference).await.get("media_edit").is_none());
        assert_eq!(
            edit(&app, &reference, "reverted", json!({"caption":"new"})).await["delivery"]
                ["outcome"],
            "rejected"
        );
        assert!(app.platform.memory.lock().unwrap().writes.is_empty());
    }
}
#[tokio::test]
async fn native_media_attempt_is_durable_before_dispatch_and_definite_rejection_restores() {
    let (app, reference) = decorated_app().await;
    let started = Arc::new(tokio::sync::Semaphore::new(0));
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    {
        let mut memory = app.platform.memory.lock().unwrap();
        memory.write_gate = Some((started.clone(), release.clone()));
        memory.reject_write = Some((1, StatusCode::BAD_REQUEST));
    }
    let request = app.effect(
        &reference,
        "update",
        "attempt",
        json!({"native_media_update":{"caption":"new"}}),
    );
    let task = tokio::spawn(async move { request.send().await.unwrap() });
    entered(&started).await;
    let (_, pending) = store(&app).load(&digest(&app, &reference)).await.unwrap();
    assert!(matches!(
        pending.status,
        Some(RevisionStatus::Attempting { .. })
    ));
    assert!(app.caps(&reference).await.get("media_edit").is_none());
    release.add_permits(1);
    assert_eq!(
        value(task.await.unwrap()).await["delivery"]["outcome"],
        "rejected"
    );
    let (_, ready) = store(&app).load(&digest(&app, &reference)).await.unwrap();
    assert!(ready.ready());
    assert_eq!(ready.revision, pending.revision);
    assert_eq!(ready.snapshot, pending.snapshot);
    assert!(app.caps(&reference).await.get("media_edit").is_some());
    assert!(fence_available(&app).await);
}
#[tokio::test]
async fn native_media_attempt_cas_failure_writes_nothing_and_holds_fence() {
    let (app, reference) = media_app().await;
    let started = Arc::new(tokio::sync::Semaphore::new(0));
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    app.platform.memory.lock().unwrap().message_gate = Some((started.clone(), release.clone()));
    let request = app.effect(
        &reference,
        "update",
        "attempt",
        json!({"native_media_update":{"caption":"new"}}),
    );
    let task = tokio::spawn(async move { request.send().await.unwrap() });
    entered(&started).await;
    app.backend.put_failures.store(1, Ordering::SeqCst);
    release.add_permits(1);
    assert_eq!(
        value(task.await.unwrap()).await["delivery"]["outcome"],
        "unknown"
    );
    assert!(app.platform.memory.lock().unwrap().writes.is_empty());
    assert!(!fence_available(&app).await);
}
#[tokio::test]
async fn native_media_rejection_cannot_restore_concurrently_retired_attempt() {
    let (app, reference) = decorated_app().await;
    let started = Arc::new(tokio::sync::Semaphore::new(0));
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    {
        let mut memory = app.platform.memory.lock().unwrap();
        memory.write_gate = Some((started.clone(), release.clone()));
        memory.reject_write = Some((1, StatusCode::BAD_REQUEST));
    }
    let request = app.effect(
        &reference,
        "update",
        "attempt",
        json!({"native_media_update":{"caption":"new"}}),
    );
    let task = tokio::spawn(async move { request.send().await.unwrap() });
    entered(&started).await;
    let store = store(&app);
    let (original, pending) = store.load(&digest(&app, &reference)).await.unwrap();
    let operation = match &pending.status {
        Some(RevisionStatus::Attempting { operation, .. }) => operation,
        _ => panic!(),
    };
    assert!(store
        .restore(&original, &pending, "another-operation")
        .await
        .is_err());
    let mut changed = pending.clone();
    changed.snapshot.caption = "changed baseline".into();
    assert!(store.restore(&original, &changed, operation).await.is_err());
    store
        .retire(&original, &pending, operation, "concurrent_retirement")
        .await
        .unwrap();
    release.add_permits(1);
    assert_eq!(
        value(task.await.unwrap()).await["delivery"]["outcome"],
        "rejected"
    );
    let (_, current) = store.load(&digest(&app, &reference)).await.unwrap();
    assert!(
        matches!(current.status,Some(RevisionStatus::Retired { ref reason,.. }) if reason == "concurrent_retirement")
    );
    assert!(!fence_available(&app).await);
}
#[tokio::test]
async fn native_media_owned_response_changes_remain_unknown_and_fenced() {
    for change in [
        json!({"content":"external"}),
        json!({"flags":0}),
        json!({"type":19}),
        json!({"embeds":[{"type":"rich","title":"authored"}]}),
    ] {
        let (app, reference) = decorated_app().await;
        app.platform.memory.lock().unwrap().media_after = Some(change);
        assert_eq!(
            edit(&app, &reference, "ambiguous", json!({"caption":"new"})).await["delivery"]
                ["outcome"],
            "unknown"
        );
        assert!(!fence_available(&app).await);
        assert!(app.caps(&reference).await.get("media_edit").is_none());
    }
}
#[tokio::test]
async fn native_media_legacy_original_hash_and_plain_edits_remain_compatible() {
    let (app, reference) = media_app().await;
    let store = store(&app);
    let (mut original, _) = store.load(&digest(&app, &reference)).await.unwrap();
    original.snapshot.presentation = None;
    for file in &mut original.snapshot.files {
        file.metadata = None;
    }
    let old = serde_json::to_value(&original).unwrap();
    assert!(old["snapshot"].get("presentation").is_none());
    assert!(old["snapshot"]["files"][0].get("metadata").is_none());
    let reread: Original = serde_json::from_value(old.clone()).unwrap();
    assert_eq!(mutation::hash(&reread), mutation::hash(&old));
    let digest = store.issue(&original).await.unwrap();
    let (_, current) = store.load(&digest).await.unwrap();
    assert!(serde_json::to_value(&current)
        .unwrap()
        .get("status")
        .is_none());
    let reference = authority::reference(&original, &current, &app.state.keys, None).unwrap();
    let receipt = edit(&app, &reference, "legacy", json!({"caption":"new"})).await;
    assert_eq!(receipt["delivery"]["outcome"], "confirmed");
    let (_, current) = store.load(&digest).await.unwrap();
    assert!(current.snapshot.presentation.is_none());
    app.platform.memory.lock().unwrap().message["embeds"] =
        json!([{"type":"link","url":"https://example.com"}]);
    assert_eq!(
        edit(
            &app,
            &next(&receipt),
            "no-upgrade",
            json!({"caption":"later"})
        )
        .await["delivery"]["outcome"],
        "rejected"
    );
    assert_eq!(app.platform.memory.lock().unwrap().writes.len(), 1);
}

#[tokio::test]
async fn native_telegram_caption_replacement_accepts_new_automatic_entities_and_retires_oversize() {
    for bounded in [true, false] {
        let (mut app, reference, platform) = telegram_app().await;
        let caption = if bounded {
            "😀 https://example.com".into()
        } else {
            "a ".repeat(129)
        };
        let mut body = telegram_body(&caption);
        body["caption_entities"] = if bounded {
            json!([{"type":"url","offset":3,"length":19}])
        } else {
            Value::Array(
                (0..129)
                    .map(|i| json!({"type":"mention","offset":2*i,"length":1}))
                    .collect(),
            )
        };
        answer_telegram(&platform, body);
        let receipt = edit(&app, &reference, "entities", json!({"caption":caption})).await;
        assert_eq!(receipt["delivery"]["outcome"], "confirmed");
        assert_eq!(receipt["message_ref"].as_str().is_some(), bounded);
        let calls = platform.calls();
        let payload: Value = serde_json::from_slice(&calls[1].body).unwrap();
        assert_eq!(payload["caption_entities"], json!([]));
        assert!(payload.get("parse_mode").is_none());
        app.restart().await;
        assert_eq!(
            edit(&app, &reference, "entities", json!({"caption":caption})).await,
            receipt
        );
        assert_eq!(platform.calls().len(), 2);
    }
}
