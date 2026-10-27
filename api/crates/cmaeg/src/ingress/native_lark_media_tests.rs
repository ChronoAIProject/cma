use super::*;

fn media(id: &str) -> Value {
    let mut raw = text(id, "", fixture::NOW);
    raw["event"]["message"]["message_type"] = "media".into();
    raw["event"]["message"]["content"] =
        json!({"file_key":"file_video","image_key":"img_cover","duration":2000})
            .to_string()
            .into();
    raw
}

#[tokio::test]
async fn media_admission_needs_both_gates_but_retained_replays_and_notifications_survive_pause() {
    for (shared, native) in [(false, false), (false, true), (true, false)] {
        let mut s = Setup::new().await;
        s.state = s
            .state
            .with_media_enabled(shared)
            .with_native_lark_media(native);
        assert!(
            matches!(s.launch(media("om_media")).await, LaunchOutcome::Refused(p) if p.detail.as_deref() == Some("native_lark_media_disabled"))
        );
    }
    let mut s = Setup::new().await;
    s.state = s
        .state
        .with_media_enabled(true)
        .with_native_lark_media(true);
    let raw = media("om_media");
    admitted(s.launch(raw.clone()).await);
    let plan = s.plan(&raw).await;
    let ActivityKind::Message { attachments, .. } = &plan.activity.kind else {
        panic!()
    };
    assert_eq!(attachments.len(), 1);
    assert!(attachments[0].thumbnail.is_some());
    assert!(
        matches!(&attachments[0].source, Some(cmaeg_core::activity::AttachmentSource::NativeLark { message_id, .. }) if message_id == "om_media")
    );
    assert!(!plan.targets.is_empty());
    s.state = s
        .state
        .with_media_enabled(false)
        .with_native_lark_media(false);
    admitted(s.launch(raw.clone()).await);
    let mut changed = raw;
    changed["event"]["message"]["content"] = json!({"file_key":"file_changed"}).to_string().into();
    assert!(matches!(s.launch(changed).await, LaunchOutcome::Refused(p) if p.status == 409));
    let reaction = reaction("media_reaction", "om_media", true, fixture::NOW);
    admitted(s.launch(reaction.clone()).await);
    let reaction_plan = s.plan(&reaction).await;
    assert_eq!(
        reaction_plan.activity.conversation,
        plan.activity.conversation
    );
    assert_eq!(
        reaction_plan
            .targets
            .iter()
            .map(|t| &t.launch.consumer)
            .collect::<Vec<_>>(),
        plan.targets
            .iter()
            .map(|t| &t.launch.consumer)
            .collect::<Vec<_>>()
    );
    let recall = recall("media_recall", "om_media", fixture::NOW);
    admitted(s.launch(recall.clone()).await);
    let recall_plan = s.plan(&recall).await;
    assert_eq!(
        recall_plan.activity.conversation,
        plan.activity.conversation
    );
    assert!(
        matches!(&recall_plan.activity.kind, ActivityKind::Event { attachments, .. } if attachments.is_empty())
    );
}
