use super::*;
use crate::whatsapp::{
    message_changes_tests::{edit, edited_text, revoke},
    message_context,
};
use cmaeg_core::channel::{Secret, Source};
impl Setup {
    pub(super) fn changes(&mut self) {
        self.state.native_whatsapp_message_events_enabled = true;
        let Source::Whatsapp { message_events, .. } =
            &mut Arc::make_mut(&mut self.channel).definition.sources[0]
        else {
            panic!()
        };
        *message_events = true;
    }
}
async fn plan(setup: &Setup, message: &str) -> super::super::routing_store::AdmittedPlan {
    setup
        .server
        .store()
        .load(&setup.identity(message))
        .await
        .unwrap()
        .unwrap()
}
#[tokio::test]
async fn native_whatsapp_changes_preserve_original_targets_rebatch_and_gate_off_replay() {
    let mut setup = Setup::new().await;
    setup.changes();
    admitted(
        setup
            .launch(setup.verified(text("wamid.original", "@reviewer original"), vec![]))
            .await,
    );
    let original = plan(&setup, "wamid.original").await;
    assert_eq!(original.targets.len(), 1);
    assert_eq!(
        original.targets[0].launch.consumer.as_str(),
        fixture::REVIEWER
    );
    let change = edited_text("wamid.edit", "wamid.original", "@lead replaced");
    let first = setup.verified(change.clone(), vec![]);
    let body = first.body.clone();
    admitted(setup.launch(first).await);
    let saved = plan(&setup, "wamid.edit").await;
    assert_eq!(saved.targets.len(), original.targets.len());
    for (changed, original) in saved.targets.iter().zip(&original.targets) {
        assert_eq!(changed.launch.consumer, original.launch.consumer);
        assert_eq!(changed.launch.provider, original.launch.provider);
        assert_eq!(
            changed.launch.event.partition_key,
            original.launch.event.partition_key
        );
        assert_eq!(
            changed.launch.event.conversation_id,
            original.launch.event.conversation_id
        );
    }
    let ActivityKind::Event { ce, attachments } = &saved.activity.kind else {
        panic!()
    };
    assert_eq!(ce.r#type, "cma.message.updated.v1");
    assert_eq!(
        ce.data.as_ref().unwrap()["patch"],
        json!({"text":"@lead replaced"})
    );
    assert_eq!(
        ce.data.as_ref().unwrap()["original_event_id"],
        original.identity.event_id.as_str()
    );
    assert_eq!(
        ce.data.as_ref().unwrap()["original_author"],
        json!(original.activity.actor)
    );
    assert!(attachments.is_empty());
    setup.state.native_whatsapp_message_events_enabled = false;
    let writes = setup.server.memory.writes.load(Ordering::SeqCst);
    admitted(
        setup
            .launch(setup.verified(change, vec![text("wamid.extra", "batch")]))
            .await,
    );
    assert_eq!(setup.server.memory.writes.load(Ordering::SeqCst), writes);
    assert_eq!(plan(&setup, "wamid.edit").await.source_body, body);
    assert!(
        matches!(setup.launch(setup.verified(edited_text("wamid.edit", "wamid.original", "changed facts"), vec![])).await, LaunchOutcome::Refused(p) if p.status==409)
    );
    assert!(
        matches!(setup.launch(setup.verified(revoke("wamid.revoke", "wamid.original"), vec![])).await, LaunchOutcome::Refused(p) if p.status==422)
    );
    setup.state.native_whatsapp_message_events_enabled = true;
    admitted(
        setup
            .launch(setup.verified(revoke("wamid.revoke", "wamid.original"), vec![]))
            .await,
    );
    let deleted = plan(&setup, "wamid.revoke").await;
    assert!(
        matches!(&deleted.activity.kind, ActivityKind::Event { ce, .. } if ce.r#type=="cma.message.deleted.v1" && ce.data.as_ref().unwrap().get("patch").is_none())
    );
}
#[tokio::test]
async fn native_whatsapp_changes_caption_patch_preserves_exact_original_media() {
    let mut setup = Setup::new().await;
    setup.changes();
    setup.state.media_enabled = true;
    setup.state.native_whatsapp_media_enabled = true;
    let mut media = crate::whatsapp::media_tests::message("image", "wamid.media", b"same media");
    media["image"]["caption"] = "before".into();
    admitted(setup.launch(setup.verified(media.clone(), vec![])).await);
    let original = plan(&setup, "wamid.media").await;
    media["image"]["caption"] = "after".into();
    media["image"]["url"] = "https://never-fetch.example/private".into();
    admitted(
        setup
            .launch(setup.verified(edit("wamid.caption", "wamid.media", media.clone()), vec![]))
            .await,
    );
    let saved = plan(&setup, "wamid.caption").await;
    let ActivityKind::Event { ce, attachments } = &saved.activity.kind else {
        panic!()
    };
    assert!(attachments.is_empty());
    assert_eq!(ce.data.as_ref().unwrap()["patch"], json!({"text":"after"}));
    assert!(!json!(saved.activity).to_string().contains("never-fetch"));
    assert_eq!(
        plan(&setup, "wamid.media").await.activity,
        original.activity
    );
    media["image"]["id"] = "99999".into();
    assert!(
        matches!(setup.launch(setup.verified(edit("wamid.replacement", "wamid.media", media), vec![])).await, LaunchOutcome::Refused(p) if p.status==403)
    );
    assert!(
        matches!(setup.launch(setup.verified(edited_text("wamid.wrong-kind", "wamid.media", "text"), vec![])).await, LaunchOutcome::Refused(p) if p.status==403)
    );
}
#[tokio::test]
async fn native_whatsapp_changes_refuse_rotations_sender_source_and_missing_originals() {
    let mut setup = Setup::new().await;
    setup.changes();
    admitted(
        setup
            .launch(setup.verified(text("wamid.original", "hello"), vec![]))
            .await,
    );
    for field in 0..6 {
        let mut change = setup.verified(revoke("wamid.revoke", "wamid.original"), vec![]);
        let channel = Arc::make_mut(&mut change.channel);
        match field {
            0 => {
                channel
                    .definition
                    .secrets
                    .whatsapp
                    .as_mut()
                    .unwrap()
                    .access_token = Secret::new("rotated-access-token")
            }
            1 => {
                channel
                    .definition
                    .secrets
                    .whatsapp
                    .as_mut()
                    .unwrap()
                    .app_secret = Secret::new("rotated-app-secret")
            }
            2 => {
                channel
                    .definition
                    .secrets
                    .whatsapp
                    .as_mut()
                    .unwrap()
                    .verify_token = Secret::new("rotated-verify-token")
            }
            3 => change.message.sender = "447700900999".into(),
            4 => {
                if let Source::Whatsapp { message_events, .. } = &mut change.source {
                    *message_events = false;
                }
            }
            _ => {
                channel.definition.sources[0] = source();
            }
        }
        assert!(
            matches!(setup.launch(change).await, LaunchOutcome::Refused(p) if p.status==403),
            "mutation {field}"
        );
    }
    let missing = setup.verified(revoke("wamid.missing", "wamid.outgoing"), vec![]);
    assert!(matches!(setup.launch(missing).await, LaunchOutcome::Refused(p) if p.status==503));
    let mut missing = setup.verified(revoke("wamid.missing", "wamid.outgoing"), vec![]);
    missing.received_at += 120;
    assert!(matches!(setup.launch(missing).await, LaunchOutcome::Refused(p) if p.status==422));
}
#[tokio::test]
async fn native_whatsapp_changes_do_not_upgrade_legacy_originals_or_gate_off_messages() {
    let mut setup = Setup::new().await;
    let message = text("wamid.legacy", "ordinary");
    admitted(setup.launch(setup.verified(message.clone(), vec![])).await);
    setup.changes();
    admitted(setup.launch(setup.verified(message, vec![])).await);
    assert!(
        matches!(setup.launch(setup.verified(revoke("wamid.revoke", "wamid.legacy"), vec![])).await, LaunchOutcome::Refused(p) if p.status==403)
    );
    setup.state.native_whatsapp_message_events_enabled = false;
    admitted(
        setup
            .launch(setup.verified(text("wamid.off", "ordinary"), vec![]))
            .await,
    );
    setup.state.native_whatsapp_message_events_enabled = true;
    assert!(
        matches!(setup.launch(setup.verified(revoke("wamid.off-revoke", "wamid.off"), vec![])).await, LaunchOutcome::Refused(p) if p.status==403)
    );
}
#[tokio::test]
async fn native_whatsapp_changes_execution_rechecks_original_credentials_and_current_consent() {
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    let mut setup = Setup::at(now).await;
    setup.changes();
    let mut original = text("wamid.original", "hello");
    original["timestamp"] = now.to_string().into();
    let mut edited = edited_text("wamid.edit", "wamid.original", "updated");
    edited["timestamp"] = now.to_string().into();
    admitted(setup.launch(setup.verified(original, vec![])).await);
    admitted(setup.launch(setup.verified(edited, vec![])).await);
    let plan = plan(&setup, "wamid.edit").await;
    let keys = Arc::new(fixture::keys());
    assert!(
        message_context::current(&setup.server.records, &keys, &setup.channel, &plan)
            .await
            .is_ok()
    );
    setup.state.native_whatsapp_message_events_enabled = false;
    assert!(
        message_context::current(&setup.server.records, &keys, &setup.channel, &plan)
            .await
            .is_ok()
    );
    let mut changed = (*setup.channel).clone();
    changed
        .definition
        .secrets
        .whatsapp
        .as_mut()
        .unwrap()
        .access_token = Secret::new("rotated-access-token");
    assert!(matches!(
        message_context::current(&setup.server.records, &keys, &changed, &plan).await,
        Err(message_context::Error::Denied)
    ));
    changed = (*setup.channel).clone();
    changed.definition.sources[0] = source();
    assert!(matches!(
        message_context::current(&setup.server.records, &keys, &changed, &plan).await,
        Err(message_context::Error::Denied)
    ));
}
#[tokio::test]
async fn native_whatsapp_changes_never_use_bot_outbound_receipt_authority() {
    let mut setup = Setup::new().await;
    setup.changes();
    admitted(
        setup
            .launch(setup.verified(text("wamid.original", "hello"), vec![]))
            .await,
    );
    let original = plan(&setup, "wamid.original").await;
    let target = &original.targets[0].launch;
    setup
        .state
        .whatsapp_receipts
        .as_ref()
        .unwrap()
        .put(&Receipt {
            kind: "native_whatsapp_receipt_v1".into(),
            group: None,
            original: original.identity.clone(),
            source: original.activity.source.clone(),
            recipient: "447700900123".into(),
            message_id: "wamid.cma-reply".into(),
            consumer: target.consumer.clone(),
            provider: target.provider.clone(),
            partition_key: target.event.partition_key.clone(),
            conversation_id: target.event.conversation_id.clone(),
            confirmed_at: setup.now,
            retained_until: setup.now + RETENTION,
        })
        .await
        .unwrap();
    for message in [
        revoke("wamid.delete-reply", "wamid.cma-reply"),
        edited_text("wamid.edit-reply", "wamid.cma-reply", "fake bot text"),
    ] {
        let mut verified = setup.verified(message, vec![]);
        verified.received_at += 120;
        assert!(matches!(setup.launch(verified).await, LaunchOutcome::Refused(p) if p.status==422));
    }
}
#[tokio::test]
async fn native_whatsapp_changes_original_context_must_commit_before_admission_and_retries_recover()
{
    let mut setup = Setup::new().await;
    setup.changes();
    let id = crate::connections::store::index_id(
        "whatsapp-message-context-v1",
        &[
            setup.channel.definition.record_id.as_str(),
            "12345",
            "23456",
            "34567",
            "447700900123",
            "wamid.original",
        ],
    );
    setup.server.fault(id, true, false);
    let original = text("wamid.original", "hello");
    assert!(
        matches!(setup.launch(setup.verified(original.clone(), vec![])).await, LaunchOutcome::Refused(p) if p.status==503)
    );
    assert!(setup
        .server
        .store()
        .load(&setup.identity("wamid.original"))
        .await
        .unwrap()
        .is_none());
    admitted(setup.launch(setup.verified(original, vec![])).await);
    admitted(
        setup
            .launch(setup.verified(revoke("wamid.revoke", "wamid.original"), vec![]))
            .await,
    );
}
