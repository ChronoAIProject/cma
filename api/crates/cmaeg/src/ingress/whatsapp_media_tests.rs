use super::*;
use crate::whatsapp::media_tests::message;

#[tokio::test]
async fn native_whatsapp_media_requires_both_issuance_flags_but_replays_retained_work_when_off() {
    for (media, native) in [(false, false), (false, true), (true, false)] {
        let mut setup = Setup::new().await;
        setup.state = setup
            .state
            .with_media_enabled(media)
            .with_whatsapp_media(native);
        let value = setup.verified(message("document", "wamid.media", b"hello"), Vec::new());
        assert!(matches!(setup.launch(value).await,LaunchOutcome::Refused(p) if p.status==422));
        assert!(setup
            .server
            .store()
            .load(&setup.identity("wamid.media"))
            .await
            .unwrap()
            .is_none());
    }
    let mut setup = Setup::new().await;
    setup.state = setup
        .state
        .with_media_enabled(true)
        .with_whatsapp_media(true);
    let raw = message("document", "wamid.media", b"hello");
    let verified = setup.verified(raw.clone(), Vec::new());
    let original_body = verified.body.clone();
    admitted(setup.launch(verified).await);
    let plan = setup
        .server
        .store()
        .load(&setup.identity("wamid.media"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(plan.source_body, original_body);
    let ActivityKind::Message { attachments, .. } = &plan.activity.kind else {
        panic!()
    };
    assert_eq!(attachments.len(), 1);
    assert_eq!(plan.activity.actor.kind, ActorKind::Unknown);
    setup.state = setup
        .state
        .with_media_enabled(true)
        .with_whatsapp_media(false);
    let writes = setup.server.memory.writes.load(Ordering::SeqCst);
    admitted(
        setup
            .launch(setup.verified(raw.clone(), vec![text("wamid.extra", "rebatch")]))
            .await,
    );
    assert_eq!(setup.server.memory.writes.load(Ordering::SeqCst), writes);
    let changed = message("document", "wamid.media", b"changed");
    assert!(
        matches!(setup.launch(setup.verified(changed,Vec::new())).await,LaunchOutcome::Refused(p) if p.status==409)
    );
    admitted(
        setup
            .launch(setup.verified(reaction("wamid.reaction", "wamid.media", "👍"), Vec::new()))
            .await,
    );
    let reaction = setup
        .server
        .store()
        .load(&setup.identity("wamid.reaction"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(plan.targets.len(), reaction.targets.len());
    let ActivityKind::Event { ce, .. } = reaction.activity.kind else {
        panic!()
    };
    assert_eq!(ce.subject.as_deref(), Some("wamid.media"));
}
