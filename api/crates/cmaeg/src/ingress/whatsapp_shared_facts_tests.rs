use super::*;
use crate::whatsapp::shared_facts::tests::{contact, place};
use cmaeg_core::activity::{ConversationKind, SourceKind};

#[tokio::test]
async fn native_whatsapp_shared_facts_keep_sender_scope_first_envelope_and_original_consumers() {
    let setup = Setup::new().await;
    for raw in [contact("wamid.contact"), place("wamid.location")] {
        let id = raw["id"].as_str().unwrap();
        let first = setup.verified(raw.clone(), Vec::new());
        let body = first.body.clone();
        admitted(setup.launch(first).await);
        let original = setup
            .server
            .store()
            .load(&setup.identity(id))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(original.source_body, body);
        assert_eq!(original.activity.actor.id, "447700900123");
        assert_eq!(original.activity.actor.kind, ActorKind::Unknown);
        assert_eq!(original.activity.conversation.id, "447700900123");
        assert_eq!(
            original.activity.conversation.kind,
            ConversationKind::Private
        );
        assert_eq!(
            original.activity.source,
            SourceKind::Whatsapp {
                app_id: "12345".into(),
                waba_id: "23456".into(),
                phone_number_id: "34567".into()
            }
        );
        let ActivityKind::Message {
            text: display,
            attachments,
            mentions_bot,
            reply_to,
            album_id,
            quoted_message: _,
        } = &original.activity.kind
        else {
            panic!()
        };
        assert!(display.starts_with("Shared "));
        assert!(!display.contains("999999") && !display.contains("127.0.0.1"));
        assert!(
            attachments.is_empty() && !mentions_bot && reply_to.is_none() && album_id.is_none()
        );
        assert_eq!(original.targets.len(), 3);
        let writes = setup.server.memory.writes.load(Ordering::SeqCst);
        let mut replay = setup.verified(raw.clone(), vec![text_fixture()]);
        Arc::make_mut(&mut replay.channel)
            .definition
            .routing
            .as_mut()
            .unwrap()
            .default_targets = vec![ConsumerId::parse("default").unwrap()];
        admitted(setup.launch(replay).await);
        assert_eq!(setup.server.memory.writes.load(Ordering::SeqCst), writes);
        let replayed = setup
            .server
            .store()
            .load(&setup.identity(id))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(replayed.source_body, original.source_body);
        assert_eq!(replayed.request_hash, original.request_hash);
        assert!(replayed.targets == original.targets);

        assert!(
            matches!(setup.launch(setup.verified(text(id, display), Vec::new())).await,
            LaunchOutcome::Refused(problem) if problem.status == 409)
        );

        let mut changed = raw;
        if changed["type"] == "contacts" {
            changed["contacts"][0]["phones"][0]["phone"] = "+1 202 555 0123".into();
        } else {
            changed["location"]["latitude"] = 2.0.into();
        }
        assert!(
            matches!(setup.launch(setup.verified(changed, Vec::new())).await,
            LaunchOutcome::Refused(problem) if problem.status == 409)
        );
    }
}

fn text_fixture() -> Value {
    text(
        "wamid.unrelated",
        "rebatching cannot replace the first envelope",
    )
}

#[tokio::test]
async fn native_whatsapp_shared_facts_never_capture_or_grant_message_change_authority() {
    use crate::whatsapp::{
        message_changes_tests::{edited_text, revoke},
        message_context,
    };
    let mut setup = Setup::new().await;
    setup.changes();
    for raw in [contact("wamid.contact"), place("wamid.location")] {
        let id = raw["id"].as_str().unwrap();
        let verified = setup.verified(raw.clone(), Vec::new());
        assert!(matches!(
            message_context::Context::capture(&verified, setup.now),
            Err(message_context::Error::Denied)
        ));
        // An earlier failed text admission can leave a context whose serialized
        // facts equal this display projection. It still cannot authorize changes.
        let webhook::Content::Text { text: display } = &verified.message.content else {
            panic!()
        };
        let twin = setup.verified(text(id, display), Vec::new());
        message_context::Context::capture(&twin, setup.now)
            .unwrap()
            .remember(
                &setup.server.records,
                &fixture::keys(),
                &setup.channel,
                &twin.source,
            )
            .await
            .unwrap();
        admitted(setup.launch(verified).await);
        let original = setup
            .server
            .store()
            .load(&setup.identity(id))
            .await
            .unwrap()
            .unwrap();
        assert!(message_context::check(
            &setup.server.records,
            &fixture::keys(),
            &setup.channel,
            &original,
            "447700900123",
            id,
            setup.now
        )
        .await
        .is_err());
        let writes = setup.server.memory.writes.load(Ordering::SeqCst);
        for change in [
            edited_text("wamid.edit-shared", id, "changed"),
            revoke("wamid.revoke-shared", id),
        ] {
            assert!(
                matches!(setup.launch(setup.verified(change, Vec::new())).await, LaunchOutcome::Refused(problem) if problem.status == 403)
            );
            assert_eq!(setup.server.memory.writes.load(Ordering::SeqCst), writes);
        }
    }
}
