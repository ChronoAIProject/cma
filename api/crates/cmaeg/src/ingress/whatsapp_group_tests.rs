use super::*;
use crate::whatsapp::groups::tests::{group_text, source_group, GROUP};
use cmaeg_core::{activity::ConversationKind, seal::ReplyAddress};

#[tokio::test]
async fn native_whatsapp_group_ingress_gate_replay_and_original_consumer_custody() {
    let mut setup = Setup::new().await;
    Arc::make_mut(&mut setup.channel).definition.sources = vec![source_group()];
    assert!(
        matches!(setup.launch(setup.verified(group_text("wamid.original"),vec![])).await,LaunchOutcome::Refused(p) if p.status==422)
    );
    assert!(setup
        .server
        .store()
        .load(&setup.identity("wamid.original"))
        .await
        .unwrap()
        .is_none());
    setup.state = setup.state.with_whatsapp_groups(true);
    admitted(
        setup
            .launch(setup.verified(group_text("wamid.original"), vec![]))
            .await,
    );
    let plan = setup
        .server
        .store()
        .load(&setup.identity("wamid.original"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(plan.activity.actor.id, "447700900123");
    assert_eq!(plan.activity.conversation.id, GROUP);
    assert_eq!(plan.activity.conversation.kind, ConversationKind::Group);
    assert_eq!(plan.targets.len(), 3);
    for target in &plan.targets {
        let event = &target.launch.event;
        assert!(
            matches!(event.reply,ReplyAddress::WhatsappGroup(ref reply) if reply.group_id==GROUP && reply.participant_id=="447700900123")
        );
    }
    setup.state = setup.state.with_whatsapp_groups(false);
    let writes = setup.server.memory.writes.load(Ordering::SeqCst);
    admitted(
        setup
            .launch(setup.verified(group_text("wamid.original"), vec![]))
            .await,
    );
    assert_eq!(setup.server.memory.writes.load(Ordering::SeqCst), writes);
    let mut changed = group_text("wamid.original");
    changed["from"] = "447700900999".into();
    assert!(
        matches!(setup.launch(setup.verified(changed,vec![])).await,LaunchOutcome::Refused(p) if p.status==409)
    );
    // A private reaction cannot acquire authority over a group message with the same wamid.
    assert!(
        matches!(setup.launch(setup.verified(reaction("wamid.reaction","wamid.original","👍"),vec![])).await,LaunchOutcome::Refused(p) if p.status==403)
    );
}
#[tokio::test]
async fn native_whatsapp_group_media_keeps_independent_issuance_gates() {
    for (media, native) in [(false, true), (true, false), (true, true)] {
        let mut setup = Setup::new().await;
        Arc::make_mut(&mut setup.channel).definition.sources = vec![source_group()];
        setup.state = setup
            .state
            .with_whatsapp_groups(true)
            .with_media_enabled(media)
            .with_whatsapp_media(native);
        let mut message = crate::whatsapp::media_tests::message("document", "wamid.pdf", b"hello");
        message["group_id"] = GROUP.into();
        let outcome = setup.launch(setup.verified(message, vec![])).await;
        if media && native {
            admitted(outcome);
        } else {
            assert!(matches!(outcome,LaunchOutcome::Refused(p) if p.status==422));
        }
    }
}
