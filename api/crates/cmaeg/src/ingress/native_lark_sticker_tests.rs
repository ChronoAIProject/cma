use super::*;
fn sticker(key: &str) -> Value {
    let mut raw = text("om_sticker", "", fixture::NOW);
    raw["event"]["message"]["message_type"] = "sticker".into();
    raw["event"]["message"]["content"] = json!({"file_key":key}).to_string().into();
    raw
}
#[tokio::test]
async fn sticker_intake_is_independent_default_off_and_retained_replay_survives_disable() {
    let mut s = Setup::new().await;
    let raw = sticker("retained-key");
    assert!(matches!(s.launch(raw.clone()).await,LaunchOutcome::Refused(p) if p.status==422));
    s.state = s.state.with_native_lark_stickers(true);
    admitted(s.launch(raw.clone()).await);
    let plan = s.plan(&raw).await;
    let ActivityKind::Message {
        attachments, text, ..
    } = &plan.activity.kind
    else {
        panic!()
    };
    assert_eq!(text, crate::native_lark::stickers::DESCRIPTION);
    assert_eq!(attachments.len(), 1);
    assert!(attachments[0].source.is_none());
    assert_eq!(
        attachments[0].variant,
        Some(cmaeg_core::activity::MediaVariant::Sticker)
    );
    assert!(!serde_json::to_string(&plan.activity)
        .unwrap()
        .contains("retained-key"));
    assert!(String::from_utf8(plan.source_body.clone())
        .unwrap()
        .contains("retained-key"));
    s.state = s.state.with_native_lark_stickers(false);
    admitted(s.launch(raw.clone()).await);
    assert!(
        matches!(s.launch(sticker("changed-key")).await,LaunchOutcome::Refused(p) if p.status==409)
    );
}
