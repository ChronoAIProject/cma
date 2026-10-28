use super::*;
use crate::native_lark::{test_support, webhook};

#[tokio::test]
async fn shared_facts_preserve_authenticated_sender_destination_and_durable_identity() {
    for (kind, content, expected) in [
        (
            "share_user",
            json!({"user_id":"ou_shared"}),
            "Shared contact: ou_shared",
        ),
        (
            "share_chat",
            json!({"chat_id":"oc_shared"}),
            "Shared group: oc_shared",
        ),
        (
            "location",
            json!({"name":"Singapore", "latitude":"1.3521", "longitude":"103.8198"}),
            "Shared location: Singapore\nLatitude: 1.3521\nLongitude: 103.8198",
        ),
    ] {
        let setup = Setup::new().await;
        let mut raw = text("om_shared_fact", "", fixture::NOW);
        raw["event"]["message"]["message_type"] = kind.into();
        raw["event"]["message"]["content"] = content.to_string().into();
        let (body, signature) = test_support::signed(&raw, fixture::NOW);
        let verify = |bytes: &[u8]| {
            webhook::verify(
                &test_support::identity(),
                &credentials(),
                &fixture::NOW.to_string(),
                "fixture_nonce",
                &signature,
                bytes,
                fixture::NOW,
            )
        };
        let webhook::Verified::Event(verified) = verify(&body).unwrap() else {
            panic!()
        };
        let mut tampered = body;
        tampered.push(b' ');
        assert!(verify(&tampered).is_err());
        admitted(setup.launch(verified.clone()).await);
        let plan = setup.plan(&verified).await;
        assert_eq!(plan.activity.actor.id, "ou_human");
        assert_eq!(plan.activity.conversation.id, "oc_native_chat");
        assert!(matches!(&plan.activity.kind,
            ActivityKind::Message { text, attachments, reply_to: None, mentions_bot: false, .. }
            if text == expected && attachments.is_empty()));
        assert_eq!(plan.targets.len(), 3);
        for target in &plan.targets {
            let ReplyAddress::NativeLark(reply) = &target.launch.event.reply else {
                panic!()
            };
            assert_eq!(reply.chat_id, "oc_native_chat");
            assert_eq!(reply.message_id, "om_shared_fact");
        }
        let mut restricted = Setup::source();
        restricted.admission = serde_json::from_value(json!({
            "type":"allowlist", "sender_ids":["ou_shared"]
        }))
        .unwrap();
        assert!(message::admission(
            &restricted,
            &plan.activity.actor.id,
            &plan.activity.conversation,
            false
        )
        .is_err());
        let writes = setup.server.memory.writes.load(Ordering::SeqCst);
        admitted(setup.launch(verified.clone()).await);
        assert_eq!(setup.server.memory.writes.load(Ordering::SeqCst), writes);
        let mut changed = verified;
        changed["event"]["message"]["message_type"] = "text".into();
        changed["event"]["message"]["content"] = json!({"text":"changed"}).to_string().into();
        assert!(
            matches!(setup.launch(changed).await, LaunchOutcome::Refused(p) if p.status == 409)
        );
    }
}
