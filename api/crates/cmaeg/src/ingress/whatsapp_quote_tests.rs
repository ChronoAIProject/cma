use super::*;
use crate::whatsapp::message_context;
use cmaeg_core::{
    channel::Secret,
    seal::{event_ref::activity_digest, ReplyAddress},
};
fn quoted() -> Value {
    let mut m = text("wamid.original", "hello");
    m["context"] = json!({"id":"wamid.parent","from":"16505551234"});
    m
}
fn gate(setup: &mut Setup, enabled: bool) {
    Arc::get_mut(setup.state.routed.as_mut().unwrap())
        .unwrap()
        .quoted_context_enabled = enabled;
}
#[tokio::test]
async fn whatsapp_quote_batch_preflight_uses_retained_version_after_the_gate_changes() {
    for initially_enabled in [false, true] {
        let mut setup = Setup::new().await;
        gate(&mut setup, initially_enabled);
        admitted(
            setup
                .launch(setup.verified(text("wamid.original", "hello"), vec![]))
                .await,
        );
        let identity = setup.identity("wamid.original");
        let before =
            serde_json::to_vec(&setup.server.store().load(&identity).await.unwrap().unwrap())
                .unwrap();
        gate(&mut setup, !initially_enabled);
        let mut conflicting = quoted();
        conflicting["context"]["id"] = "wamid.other".into();
        let batch = setup.verified(quoted(), vec![conflicting]);
        assert_eq!(
            setup
                .state
                .routed
                .as_ref()
                .unwrap()
                .whatsapp_quote_batch_conflict(&setup.channel, &batch.source, &batch.body)
                .await
                .unwrap(),
            initially_enabled
        );
        if initially_enabled {
            assert!(matches!(
                setup.launch(batch).await,
                LaunchOutcome::Refused(problem) if problem.status == 409
            ));
        } else {
            admitted(setup.launch(batch).await);
        }
        assert_eq!(
            serde_json::to_vec(&setup.server.store().load(&identity).await.unwrap().unwrap())
                .unwrap(),
            before
        );
    }
}

#[tokio::test]
async fn whatsapp_quote_version_preserves_legacy_bytes_credential_proofs_and_gate_off_replay() {
    for initially_enabled in [false, true] {
        let mut setup = Setup::new().await;
        setup.changes();
        gate(&mut setup, initially_enabled);
        let first = setup.verified(quoted(), vec![]);
        let facts = serde_json::to_vec(&first.message).unwrap();
        let proof =
            serde_json::to_vec(&message_context::Context::capture(&first, setup.now).unwrap())
                .unwrap();
        admitted(setup.launch(first).await);
        let original = setup
            .server
            .store()
            .load(&setup.identity("wamid.original"))
            .await
            .unwrap()
            .unwrap();
        let bytes = serde_json::to_vec(&original).unwrap();
        assert_eq!(original.quote_projection, initially_enabled.then_some(1));
        assert!(
            matches!(&original.activity.kind,ActivityKind::Message{quoted_message,..} if quoted_message.is_some()==initially_enabled)
        );
        assert_eq!(original.activity.actor.id, "447700900123");
        assert_eq!(original.activity.conversation.id, "447700900123");
        assert_eq!(original.targets.len(), 3);
        assert!(
            matches!(&original.targets[0].launch.event.reply,ReplyAddress::Whatsapp{message_id,..} if message_id=="wamid.original")
        );
        for target in &original.targets {
            assert!(target.launch.event.verifies_activity(&original.activity));
        }
        gate(&mut setup, !initially_enabled);
        let again = setup.verified(quoted(), vec![text("wamid.extra", "rebatch")]);
        assert_eq!(serde_json::to_vec(&again.message).unwrap(), facts);
        assert_eq!(
            serde_json::to_vec(&message_context::Context::capture(&again, setup.now).unwrap())
                .unwrap(),
            proof
        );
        admitted(setup.launch(again).await);
        let saved = setup
            .server
            .store()
            .load(&setup.identity("wamid.original"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(serde_json::to_vec(&saved).unwrap(), bytes);
        message_context::check(
            &setup.server.records,
            &fixture::keys(),
            &setup.channel,
            &saved,
            "447700900123",
            "wamid.original",
            setup.now,
        )
        .await
        .unwrap();
        let mut changed = quoted();
        changed["context"]["id"] = "wamid.changed".into();
        let outcome = setup.launch(setup.verified(changed, vec![])).await;
        if initially_enabled {
            assert!(matches!(outcome,LaunchOutcome::Refused(p) if p.status==409));
        } else {
            admitted(outcome);
        }
        let mut tampered = saved.activity.clone();
        if let ActivityKind::Message {
            quoted_message: Some(q),
            ..
        } = &mut tampered.kind
        {
            q.message_id = "wamid.tampered".into();
            assert_ne!(activity_digest(&tampered), activity_digest(&saved.activity));
            assert!(!saved.targets[0].launch.event.verifies_activity(&tampered));
        }
        let mut rotated = setup.channel.clone();
        Arc::make_mut(&mut rotated)
            .definition
            .secrets
            .whatsapp
            .as_mut()
            .unwrap()
            .access_token = Secret::new("rotated-access-token");
        assert!(message_context::check(
            &setup.server.records,
            &fixture::keys(),
            &rotated,
            &saved,
            "447700900123",
            "wamid.original",
            setup.now
        )
        .await
        .is_err());
        assert!(message_context::check(
            &setup.server.records,
            &fixture::keys(),
            &setup.channel,
            &saved,
            "447700900999",
            "wamid.original",
            setup.now
        )
        .await
        .is_err());
    }
}
