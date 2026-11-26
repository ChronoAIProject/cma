//! Real tool orchestration against local provider and CAS record boundaries.
use super::*;
use crate::native_media::{
    authority,
    model::{Current, Original},
    store::Store,
};
use cmaeg_core::message::NativeMediaKind;
use std::sync::atomic::Ordering;
#[path = "native_media_decoration_tests.rs"]
mod decoration_tests;
#[path = "native_media_send_tests.rs"]
mod send_tests;

async fn media_app() -> (App, MessageRef) {
    let mut app = App::new().await;
    app.activity.conversation.kind = cmaeg_core::activity::ConversationKind::Group;
    app.event.activity_sha256 = Some(activity_digest(&app.activity));
    app.event.conversation_id = cmaeg_core::ids::ConversationId::derive(
        &app.state.keys,
        &app.event.channel_id,
        app.definition.partition.version,
        &app.event.partition_key,
    );
    app.state.native_media_edits = Some(Arc::new(crate::native_media::Client::new(
        Arc::new(
            crate::telegram::TelegramClient::new("http://127.0.0.1:1", Duration::from_secs(1))
                .unwrap(),
        ),
        app.platform.api.clone(),
        false,
        false,
    )));
    {
        let mut m = app.platform.memory.lock().unwrap();
        m.media_patch = true;
        m.message = json!({"id":"800","channel_id":"99","author":{"id":"77","bot":true},"type":0,"content":"original visible text","attachments":[{"id":"900","filename":"report.bin","content_type":"application/octet-stream","size":4,"description":"original alt text"}],"flags":0});
        m.guild = json!({"id":"99","type":0,"guild_id":"88","parent_id":null});
    }
    app.restart().await;
    let channel = app
        .state
        .resolver
        .resolve_current(app.event.channel_id.as_str())
        .await
        .unwrap();
    let snapshot = crate::discord::rest::media_edit::snapshot(
        &app.platform.memory.lock().unwrap().message,
        "77",
        "99",
        "800",
        NativeMediaKind::File,
    )
    .unwrap();
    let observed = now();
    let original = Original {
        event: app.event.clone(),
        activity: app.activity.clone(),
        owner: channel.definition.owner.subject.clone(),
        authority_sha256: authority::custody(&channel, &app.event, &app.activity).unwrap(),
        discord_channel: Some(json!({"id":"99","type":0,"guild_id":"88","parent_id":null})),
        message_id: "800".into(),
        snapshot: snapshot.clone(),
        confirmed_at: observed,
        expires_at: observed + 86_400,
    };
    let store = Store::new(app.state.resolver.record_client(), app.state.keys.clone());
    let digest = store.issue(&original).await.unwrap();
    let current = Current {
        original_sha256: digest,
        revision: 0,
        snapshot,
        last_operation: None,
        status: None,
        expires_at: original.expires_at,
        version: 0,
    };
    let reference = authority::reference(&original, &current, &app.state.keys, None).unwrap();
    (app, reference)
}
async fn edit(app: &App, reference: &MessageRef, key: &str, patch: Value) -> Value {
    value(
        app.effect(
            reference,
            "update",
            key,
            json!({"native_media_update":patch}),
        )
        .send()
        .await
        .unwrap(),
    )
    .await
}
fn next(receipt: &Value) -> MessageRef {
    receipt["message_ref"].as_str().unwrap().parse().unwrap()
}

#[tokio::test]
async fn native_media_description_content_revision_and_durable_replay() {
    let (mut app, reference) = media_app().await;
    let caps = app.caps(&reference).await;
    assert_eq!(caps["media_edit"]["content"], true);
    assert_eq!(caps["delete"], false);
    let first = edit(&app, &reference, "alt", json!({"caption":""})).await;
    assert_eq!(first["delivery"]["outcome"], "confirmed");
    let updated = next(&first);
    let original_expiry = reference
        .open(&app.state.keys)
        .unwrap()
        .payload
        .native_media_edit
        .unwrap()
        .expires_at;
    let a = updated
        .open(&app.state.keys)
        .unwrap()
        .payload
        .native_media_edit
        .unwrap();
    assert_eq!(a.revision, 1);
    assert_eq!(a.expires_at, original_expiry);
    assert_eq!(
        app.platform.memory.lock().unwrap().message["content"],
        "original visible text"
    );
    app.restart().await;
    assert_eq!(
        edit(&app, &reference, "alt", json!({"caption":""})).await,
        first
    );
    let stale = edit(&app, &reference, "new-key", json!({"text":"stale"})).await;
    assert_eq!(stale["delivery"]["outcome"], "rejected");
    assert_eq!(app.platform.memory.lock().unwrap().writes.len(), 1);
    let second = edit(
        &app,
        &updated,
        "content",
        json!({"text":"@everyone updated"}),
    )
    .await;
    assert_eq!(second["delivery"]["outcome"], "confirmed");
    let m = app.platform.memory.lock().unwrap();
    let description: Value = serde_json::from_slice(&m.bodies[0].1).unwrap();
    assert!(description.get("content").is_none());
    assert_eq!(description["attachments"][0]["id"], "900");
    let content: Value = serde_json::from_slice(&m.bodies[1].1).unwrap();
    assert!(content.get("attachments").is_none());
    assert_eq!(
        content["allowed_mentions"],
        json!({"parse":[],"replied_user":false})
    );
}
#[tokio::test]
async fn native_media_replacement_is_one_multipart_patch_with_final_membership() {
    let (app, reference) = media_app().await;
    let patch = json!({"replacement":{"kind":"file","source":{"type":"base64","data":"AQIDBA=="},"filename":"next.bin","mime_type":"application/octet-stream"}});
    let result = edit(&app, &reference, "replace", patch).await;
    assert_eq!(result["delivery"]["outcome"], "confirmed");
    let m = app.platform.memory.lock().unwrap();
    assert_eq!(
        m.writes,
        vec![(Method::PATCH, "/api/v10/channels/99/messages/800".into())]
    );
    assert!(m.bodies[0].0.starts_with("multipart/form-data; boundary="));
    let wire = String::from_utf8_lossy(&m.bodies[0].1);
    assert!(wire.contains("name=\"files[0]\""));
    assert!(wire.contains("name=\"payload_json\""));
    assert!(wire.contains("\"id\":0"));
    assert!(!wire.contains("\"id\":\"900\""));
    assert!(m.bodies[0].1.windows(4).any(|v| v == [1, 2, 3, 4]));
    assert_eq!(
        m.message["attachments"][0]["description"],
        "original alt text"
    );
    assert_eq!(m.message["content"], "original visible text");
}
#[tokio::test]
async fn native_media_unknown_or_post_write_storage_loss_fences_new_keys_after_restart() {
    for storage in [false, true] {
        let (mut app, reference) = media_app().await;
        if storage {
            app.platform.memory.lock().unwrap().fail_receipt = Some(app.backend.fail_put.clone());
        } else {
            app.platform.memory.lock().unwrap().media_wrong_response = true;
        }
        let first = edit(&app, &reference, "first", json!({"caption":"changed"})).await;
        assert_eq!(first["delivery"]["outcome"], "unknown");
        app.backend.fail_put.store(false, Ordering::SeqCst);
        app.platform.memory.lock().unwrap().fail_receipt = None;
        app.platform.memory.lock().unwrap().media_wrong_response = false;
        app.restart().await;
        let replay = edit(&app, &reference, "first", json!({"caption":"changed"})).await;
        assert_eq!(replay["delivery"]["outcome"], "unknown");
        let next = edit(&app, &reference, "different", json!({"caption":"other"})).await;
        assert_eq!(next["delivery"]["outcome"], "unknown");
        assert_eq!(app.platform.memory.lock().unwrap().writes.len(), 1);
    }
}
#[tokio::test]
async fn native_media_final_custody_check_refuses_mutations_during_message_read() {
    for changed in [
        "source",
        "owner",
        "provider",
        "key",
        "native_token",
        "consumer_credential",
    ] {
        let (mut app, reference) = media_app().await;
        let entered = Arc::new(tokio::sync::Semaphore::new(0));
        let release = Arc::new(tokio::sync::Semaphore::new(0));
        app.platform.memory.lock().unwrap().message_gate = Some((entered.clone(), release.clone()));
        let request = app.effect(
            &reference,
            "update",
            "race",
            json!({"native_media_update":{"caption":"changed"}}),
        );
        let update = tokio::spawn(async move { request.send().await.unwrap() });
        tokio::time::timeout(Duration::from_secs(3), entered.acquire())
            .await
            .unwrap()
            .unwrap()
            .forget();
        match changed {
            "source" => app.definition.sources.clear(),
            "owner" => app.definition.owner.subject = "new-owner".into(),
            "provider" => definition_fixture::replace_default_binding(
                &mut app.definition,
                "bnd_changed".parse().unwrap(),
            ),
            "key" => app.definition.secrets.agent_key_hash = format!("sha256:{}", "1".repeat(64)),
            "native_token" => {
                app.definition.secrets.discord_bot_token =
                    Some(cmaeg_core::channel::Secret::new("rotated-fixture-token"))
            }
            _ => definition_fixture::replace_default_credential(
                &mut app.definition,
                Some("changed-provider-fixture"),
            ),
        }
        app.update().await;
        release.add_permits(1);
        assert_eq!(
            value(update.await.unwrap()).await["delivery"]["outcome"],
            "rejected",
            "{changed}"
        );
        assert!(
            app.platform.memory.lock().unwrap().writes.is_empty(),
            "{changed}"
        );
    }
}
#[tokio::test]
async fn native_media_competing_keys_share_the_existing_effect_fence() {
    let (app, reference) = media_app().await;
    let entered = Arc::new(tokio::sync::Semaphore::new(0));
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    app.platform.memory.lock().unwrap().message_gate = Some((entered.clone(), release.clone()));
    let request = app.effect(
        &reference,
        "update",
        "first",
        json!({"native_media_update":{"caption":"changed"}}),
    );
    let pending = tokio::spawn(async move { request.send().await.unwrap() });
    tokio::time::timeout(Duration::from_secs(3), entered.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    let held = mutation::Store::discord(app.state.resolver.record_client(), app.state.keys.clone())
        .acquire(
            mutation::destination("77", "99", "800"),
            &app.definition.owner.subject,
            now() + 86_400,
        )
        .await
        .unwrap();
    assert!(
        held.is_none(),
        "the existing delete/reaction domain is held"
    );
    assert_eq!(
        edit(&app, &reference, "second", json!({"text":"second"})).await["delivery"]["outcome"],
        "unknown"
    );
    release.add_permits(1);
    assert_eq!(
        value(pending.await.unwrap()).await["delivery"]["outcome"],
        "confirmed"
    );
    assert_eq!(app.platform.memory.lock().unwrap().writes.len(), 1);
}
#[tokio::test]
async fn native_media_refuses_old_authority_missing_keys_and_external_attachment_changes() {
    let (app, reference) = media_app().await;
    let mut old = reference.open(&app.state.keys).unwrap().payload;
    old.native_media_edit = None;
    let old = MessageRef::seal(&old, &app.state.keys).unwrap();
    assert_eq!(
        app.effect(
            &old,
            "update",
            "old",
            json!({"native_media_update":{"caption":"x"}})
        )
        .send()
        .await
        .unwrap()
        .status(),
        StatusCode::FORBIDDEN
    );
    let missing = reqwest::Client::new()
        .post(format!(
            "{}/v1/messages/{}/update",
            app.base,
            reference.as_str()
        ))
        .bearer_auth(&app.key)
        .json(&json!({"native_media_update":{"caption":"x"}}))
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::BAD_REQUEST);
    app.platform.memory.lock().unwrap().message["attachments"][0]["id"] = "901".into();
    assert_eq!(
        edit(&app, &reference, "changed", json!({"caption":"x"})).await["delivery"]["outcome"],
        "rejected"
    );
    assert!(app.platform.memory.lock().unwrap().writes.is_empty());
}

async fn telegram_app() -> (App, MessageRef, crate::telegram::media_fixture::Fixture) {
    let (mut app, _) = media_app().await;
    let platform = crate::telegram::media_fixture::Fixture::new().await;
    app.definition.sources = vec![Source::Telegram {
        topic_lifecycle: false,
        topic_creation: false,
        message_events: false,
        channel_posts: false,
        member_join_chat_ids: Vec::new(),
        bot_username: None,
        bot_id: "77".into(),
        admission: AdmissionPolicy::All {},
    }];
    app.definition.secrets.telegram_bot_token = Some(cmaeg_core::channel::Secret::new(format!(
        "77:{}",
        uuid::Uuid::new_v4().simple()
    )));
    app.definition.secrets.telegram_webhook_secret = Some(cmaeg_core::channel::Secret::new(
        uuid::Uuid::new_v4().simple().to_string(),
    ));
    app.activity.source = SourceKind::Telegram {
        platform: cmaeg_core::channel::Platform::Telegram,
        bot_id: "77".into(),
    };
    app.activity.actor.id = "42".into();
    app.activity.conversation.id = "-42".into();
    app.activity.conversation.thread_id = Some("7".into());
    app.activity.event_id = "telegram:77:700".parse().unwrap();
    app.event.event_id = app.activity.event_id.clone();
    app.event.reply = ReplyAddress::Telegram {
        bot_id: "77".into(),
        chat_id: "-42".into(),
        thread_id: Some("7".into()),
        message_id: "700".into(),
    };
    app.event.telegram_admission = Some(cmaeg_core::seal::TelegramAdmissionProof {
        actor_id: "42".into(),
        actor_kind: cmaeg_core::activity::ActorKind::Human,
        conversation_kind: cmaeg_core::activity::ConversationKind::Group,
        mentioned: Some(true),
        replied_to_bot: Some(false),
    });
    app.event.activity_sha256 = Some(activity_digest(&app.activity));
    app.event.conversation_id = cmaeg_core::ids::ConversationId::derive(
        &app.state.keys,
        &app.event.channel_id,
        app.definition.partition.version,
        &app.event.partition_key,
    );
    app.state.native_media_edits = Some(Arc::new(crate::native_media::Client::new(
        Arc::new(platform.api.clone()),
        app.platform.api.clone(),
        false,
        false,
    )));
    app.update().await;
    app.restart().await;
    let channel = app
        .state
        .resolver
        .resolve_current(app.event.channel_id.as_str())
        .await
        .unwrap();
    let body = telegram_body("old caption");
    let snapshot = crate::telegram::media_edit::snapshot(
        &body,
        "77",
        "-42",
        Some("7"),
        "800",
        NativeMediaKind::File,
    )
    .unwrap();
    let observed = now();
    let original = Original {
        event: app.event.clone(),
        activity: app.activity.clone(),
        owner: channel.definition.owner.subject.clone(),
        authority_sha256: authority::custody(&channel, &app.event, &app.activity).unwrap(),
        discord_channel: None,
        message_id: "800".into(),
        snapshot: snapshot.clone(),
        confirmed_at: observed,
        expires_at: observed + 86_400,
    };
    let digest = Store::new(app.state.resolver.record_client(), app.state.keys.clone())
        .issue(&original)
        .await
        .unwrap();
    let current = Current {
        original_sha256: digest,
        revision: 0,
        snapshot,
        last_operation: None,
        status: None,
        expires_at: original.expires_at,
        version: 0,
    };
    let reference = authority::reference(&original, &current, &app.state.keys, None).unwrap();
    (app, reference, platform)
}
fn telegram_body(caption: &str) -> Value {
    json!({"message_id":800,"chat":{"id":-42},"message_thread_id":7,"from":{"id":77,"is_bot":true},"caption":caption,
        "document":{"file_id":"locator","file_unique_id":"stable","file_name":"report.bin","mime_type":"application/octet-stream","file_size":4}})
}
fn answer_telegram(platform: &crate::telegram::media_fixture::Fixture, body: Value) {
    use crate::telegram::media_fixture::Answer;
    platform.answer(Answer::json(
        StatusCode::OK,
        json!({"ok":true,"result":{"id":77,"is_bot":true,"first_name":"Fixture"}}),
    ));
    platform.answer(Answer::json(
        StatusCode::OK,
        json!({"ok":true,"result":body}),
    ));
}
#[tokio::test]
async fn native_telegram_media_caption_and_same_kind_replacement_keep_topic_and_fixed_expiry() {
    let (app, reference, platform) = telegram_app().await;
    answer_telegram(&platform, telegram_body(""));
    let caption = edit(&app, &reference, "caption", json!({"caption":""})).await;
    assert_eq!(caption["delivery"]["outcome"], "confirmed");
    assert_eq!(
        edit(&app, &reference, "caption", json!({"caption":""})).await,
        caption
    );
    assert_eq!(platform.calls().len(), 2);
    answer_telegram(&platform, telegram_body(""));
    let replacement=edit(&app,&next(&caption),"replacement",json!({"replacement":{"kind":"file","source":{"type":"base64","data":"AQIDBA=="},"filename":"report.bin","mime_type":"application/octet-stream"}})).await;
    assert_eq!(replacement["delivery"]["outcome"], "confirmed");
    let a = next(&replacement)
        .open(&app.state.keys)
        .unwrap()
        .payload
        .native_media_edit
        .unwrap();
    assert_eq!(a.revision, 2);
    assert_eq!(
        a.expires_at,
        reference
            .open(&app.state.keys)
            .unwrap()
            .payload
            .native_media_edit
            .unwrap()
            .expires_at
    );
    let calls = platform.calls();
    assert_eq!(calls.len(), 4);
    assert!(calls[1].path.ends_with("/editMessageCaption"));
    assert!(calls[3].path.ends_with("/editMessageMedia"));
    assert!(!String::from_utf8_lossy(&calls[3].body).contains("message_thread_id"));
}
#[tokio::test]
async fn native_telegram_media_changed_file_on_caption_edit_stays_unknown_across_restart() {
    let (mut app, reference, platform) = telegram_app().await;
    let mut body = telegram_body("changed");
    body["document"]["file_unique_id"] = "external_file".into();
    answer_telegram(&platform, body);
    assert_eq!(
        edit(&app, &reference, "caption", json!({"caption":"changed"})).await["delivery"]
            ["outcome"],
        "unknown"
    );
    app.restart().await;
    assert_eq!(
        edit(&app, &reference, "new-key", json!({"caption":"another"})).await["delivery"]
            ["outcome"],
        "unknown"
    );
    assert_eq!(platform.calls().len(), 2);
}
#[tokio::test]
async fn native_telegram_media_rejects_visible_text_before_provider_reads() {
    let (app, reference, platform) = telegram_app().await;
    let response = app
        .effect(
            &reference,
            "update",
            "text",
            json!({"native_media_update":{"text":"unsupported"}}),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(platform.calls().is_empty());
}

#[tokio::test]
async fn native_media_named_consumer_scope_never_falls_back_to_default() {
    let (mut app, default_ref) = media_app().await;
    let store = Store::new(app.state.resolver.record_client(), app.state.keys.clone());
    let digest = default_ref
        .open(&app.state.keys)
        .unwrap()
        .payload
        .native_media_edit
        .unwrap()
        .original_sha256;
    let (mut original, _) = store.load(&digest).await.unwrap();
    let id = routing_fixture::REVIEWER;
    let key = routing_fixture::agent_key(id);
    let mut definition = app.definition.to_blob_json();
    definition["routing"] = json!({"version":1,"default_consumer":{"name":"lead"},"consumers":[routing_fixture::consumer(id,"reviewer","cma_codex")],"default_targets":["default"]});
    definition["secrets"]["consumer_keys"] = json!([[id, key]]);
    app.definition = serde_json::from_value(definition).unwrap();
    app.update().await;
    original.event.consumer = Some(cmaeg_core::ids::ConsumerId::parse(id).unwrap());
    let channel = app
        .state
        .resolver
        .resolve_current(app.event.channel_id.as_str())
        .await
        .unwrap();
    original.authority_sha256 =
        authority::custody(&channel, &original.event, &original.activity).unwrap();
    let digest = store.issue(&original).await.unwrap();
    let current = Current {
        original_sha256: digest,
        revision: 0,
        snapshot: original.snapshot.clone(),
        last_operation: None,
        status: None,
        expires_at: original.expires_at,
        version: 0,
    };
    let reference = authority::reference(&original, &current, &app.state.keys, None).unwrap();
    assert_eq!(
        app.effect(
            &reference,
            "update",
            "borrowed",
            json!({"native_media_update":{"caption":"x"}})
        )
        .send()
        .await
        .unwrap()
        .status(),
        StatusCode::FORBIDDEN
    );
    app.key = key;
    assert_eq!(
        edit(&app, &reference, "owned", json!({"caption":"named"})).await["delivery"]["outcome"],
        "confirmed"
    );
    app.definition.routing = None;
    app.definition.secrets.consumer_keys.clear();
    app.update().await;
    assert_eq!(
        app.effect(
            &reference,
            "update",
            "owned",
            json!({"native_media_update":{"caption":"named"}})
        )
        .send()
        .await
        .unwrap()
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(app.platform.memory.lock().unwrap().writes.len(), 1);
}

#[tokio::test]
async fn native_media_key_payload_conflicts_and_explicit_rejections_do_not_resend() {
    let (app, reference) = media_app().await;
    app.platform.memory.lock().unwrap().reject_write = Some((1, StatusCode::BAD_REQUEST));
    let rejected = edit(&app, &reference, "rejected", json!({"caption":"x"})).await;
    assert_eq!(rejected["delivery"]["outcome"], "rejected");
    assert_eq!(
        edit(&app, &reference, "rejected", json!({"caption":"x"})).await,
        rejected
    );
    assert_eq!(
        app.effect(
            &reference,
            "update",
            "rejected",
            json!({"native_media_update":{"caption":"different"}})
        )
        .send()
        .await
        .unwrap()
        .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(app.platform.memory.lock().unwrap().writes.len(), 1);
    app.platform.memory.lock().unwrap().reject_write = None;
    assert_eq!(
        edit(
            &app,
            &reference,
            "next-operation",
            json!({"caption":"accepted"})
        )
        .await["delivery"]["outcome"],
        "confirmed"
    );
    assert_eq!(app.platform.memory.lock().unwrap().writes.len(), 2);
}

#[tokio::test]
async fn native_telegram_media_same_bot_token_rotation_requires_fresh_identity_but_keeps_authority()
{
    let (mut app, reference, platform) = telegram_app().await;
    app.definition.secrets.telegram_bot_token = Some(cmaeg_core::channel::Secret::new(format!(
        "77:{}",
        uuid::Uuid::new_v4().simple()
    )));
    app.update().await;
    answer_telegram(&platform, telegram_body("rotated"));
    let receipt = edit(&app, &reference, "rotated", json!({"caption":"rotated"})).await;
    assert_eq!(receipt["delivery"]["outcome"], "confirmed");
    assert_eq!(platform.calls().len(), 2);
    assert!(platform.calls()[0].path.ends_with("/getMe"));
    let original = reference
        .open(&app.state.keys)
        .unwrap()
        .payload
        .native_media_edit
        .unwrap();
    let updated = next(&receipt)
        .open(&app.state.keys)
        .unwrap()
        .payload
        .native_media_edit
        .unwrap();
    assert_eq!(updated.expires_at, original.expires_at);
    assert_eq!(updated.revision, original.revision + 1);
}

#[tokio::test]
async fn telegram_topic_enrollment_keeps_legacy_media_edits_and_exact_effect_custody() {
    let (mut app, reference, platform) = telegram_app().await;
    assert!(app.definition.routing.is_none());
    let channel = app
        .state
        .resolver
        .resolve_current(app.event.channel_id.as_str())
        .await
        .unwrap();
    let mut base = reference.open(&app.state.keys).unwrap().payload;
    base.native_media_edit = None;
    let base = MessageRef::seal(&base, &app.state.keys).unwrap();
    let effect = crate::tools::message_effects::authority::enrich(
        &base,
        &app.event,
        &channel,
        &app.state.keys,
        now(),
        false,
    )
    .unwrap()
    .open(&app.state.keys)
    .unwrap()
    .payload;
    let media = reference.open(&app.state.keys).unwrap().payload;
    let original = Store::new(app.state.resolver.record_client(), app.state.keys.clone())
        .load(&media.native_media_edit.as_ref().unwrap().original_sha256)
        .await
        .unwrap()
        .0;
    let caller = crate::auth::AgentCaller {
        key_hash: *channel.consumer_key_hash(None).unwrap(),
    };
    let mut enrolled = app.definition.clone();
    enrolled.routing = Some(crate::telegram::messaging_compat::default_routing());
    let Source::Telegram {
        topic_lifecycle, ..
    } = &mut enrolled.sources[0]
    else {
        panic!()
    };
    *topic_lifecycle = true;
    for defect in [
        "none",
        "name",
        "consumers",
        "targets",
        "rules",
        "publish",
        "non_human",
        "reply",
        "addressing",
        "provider",
        "key",
        "credential",
        "source",
    ] {
        app.definition = enrolled.clone();
        let routing = app.definition.routing.as_mut().unwrap();
        match defect {
            "none" => (),
            "name" => {
                routing.default_consumer.name = serde_json::from_value(json!("renamed")).unwrap()
            }
            "consumers" => {
                routing.consumers = crate::channels::routing_fixture::definition(
                    crate::channels::routing_fixture::routed(),
                )
                .routing
                .unwrap()
                .consumers
            }
            "targets" => routing.default_targets.clear(),
            "rules" => {
                routing.rules = serde_json::from_value(
                    json!([{"id":"rul_000000000001","when":{},"targets":["default"]}]),
                )
                .unwrap()
            }
            "publish" => routing.default_consumer.publish.enabled = true,
            "non_human" => routing.default_consumer.non_human = Some(false),
            "reply" => routing.default_consumer.reply.auto_final = Some(false),
            "addressing" => routing.addressing.mentions = !routing.addressing.mentions,
            "provider" => definition_fixture::replace_default_binding(
                &mut app.definition,
                "binding-changed".parse().unwrap(),
            ),
            "key" => definition_fixture::replace_default_agent_key(
                &mut app.definition,
                "nyxid_ag_changed-fixture",
            ),
            "credential" => definition_fixture::replace_default_credential(
                &mut app.definition,
                Some("changed-fixture"),
            ),
            "source" => app.definition.sources.clear(),
            _ => unreachable!(),
        }
        // Replace the encrypted definition exactly, including intentionally invalid routing,
        // to prove retained readers cannot broaden the fallback.
        app.backend.seed(
            app.definition.record_id.as_str(),
            &app.definition.owner.subject,
            seal_blob(
                &app.state.keys,
                &serde_json::to_vec(&app.definition.to_blob_json()).unwrap(),
            )
            .unwrap(),
        );
        let current = app
            .state
            .resolver
            .resolve_current(app.event.channel_id.as_str())
            .await;
        if defect == "none" {
            let current = current.unwrap();
            authority::check(&current, &media, &original, caller).unwrap();
            crate::tools::message_effects::authority::check(&effect, &current, caller, now())
                .unwrap();
        } else if let Ok(current) = current {
            assert!(
                authority::check(&current, &media, &original, caller).is_err(),
                "{defect}"
            );
            // Legacy sent effects did not bind provider credentials, but media custody does.
            if defect != "credential" {
                assert!(
                    crate::tools::message_effects::authority::check(
                        &effect,
                        &current,
                        caller,
                        now()
                    )
                    .is_err(),
                    "{defect}"
                );
            }
        }
    }
    app.definition = enrolled;
    app.backend.seed(
        app.definition.record_id.as_str(),
        &app.definition.owner.subject,
        seal_blob(
            &app.state.keys,
            &serde_json::to_vec(&app.definition.to_blob_json()).unwrap(),
        )
        .unwrap(),
    );
    answer_telegram(&platform, telegram_body("after enrollment"));
    let first = edit(
        &app,
        &reference,
        "topic-enrollment",
        json!({"caption":"after enrollment"}),
    )
    .await;
    assert_eq!(first["delivery"]["outcome"], "confirmed");
    let Source::Telegram {
        topic_lifecycle, ..
    } = &mut app.definition.sources[0]
    else {
        panic!()
    };
    *topic_lifecycle = false;
    app.backend.seed(
        app.definition.record_id.as_str(),
        &app.definition.owner.subject,
        seal_blob(
            &app.state.keys,
            &serde_json::to_vec(&app.definition.to_blob_json()).unwrap(),
        )
        .unwrap(),
    );
    answer_telegram(&platform, telegram_body("after opt-out"));
    assert_eq!(
        edit(
            &app,
            &next(&first),
            "topic-opt-out",
            json!({"caption":"after opt-out"})
        )
        .await["delivery"]["outcome"],
        "confirmed"
    );
    assert_eq!(platform.calls().len(), 4);
}

#[tokio::test]
async fn explicit_card_attachment_clear_refuses_standalone_media_references_before_native_io() {
    let (app, reference) = media_app().await;
    let response = app
        .effect(
            &reference,
            "update",
            "clear-unsupported",
            json!({
                "text":"Remove files", "clear_native_attachments":true
            }),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    assert!(response
        .json::<Value>()
        .await
        .unwrap()
        .to_string()
        .contains("native_attachment_clear_unsupported"));
    assert!(app.platform.memory.lock().unwrap().writes.is_empty());
}
