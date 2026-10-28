use super::*;
#[tokio::test]
async fn native_lark_parent_context_is_versioned_id_only_and_retains_original_selection() {
    for enabled in [false, true] {
        let mut setup = Setup::new().await;
        Arc::get_mut(setup.state.routed.as_mut().unwrap())
            .unwrap()
            .quoted_context_enabled = enabled;
        let mut raw = text("om_original", "hello", fixture::NOW);
        raw["event"]["message"]["parent_id"] = "om_parent".into();
        raw["event"]["message"]["root_id"] = "om_root".into();
        admitted(setup.launch(raw.clone()).await);
        let original = setup.plan(&raw).await;
        assert_eq!(original.quote_projection, enabled.then_some(1));
        if let ActivityKind::Message {
            quoted_message,
            reply_to,
            text,
            ..
        } = &original.activity.kind
        {
            assert_eq!(text, "hello");
            assert!(reply_to.is_none());
            assert_eq!(
                quoted_message.as_ref().map(|q| q.message_id.as_str()),
                enabled.then_some("om_parent")
            );
            assert!(quoted_message.as_ref().is_none_or(|q| q.content.is_none()));
        } else {
            panic!()
        }
        assert_eq!(original.targets.len(), 3);
        assert!(!String::from_utf8_lossy(&original.source_body).contains("parent"));
        let before = serde_json::to_vec(&original).unwrap();
        Arc::get_mut(setup.state.routed.as_mut().unwrap())
            .unwrap()
            .quoted_context_enabled = !enabled;
        admitted(setup.launch(raw.clone()).await);
        assert_eq!(serde_json::to_vec(&setup.plan(&raw).await).unwrap(), before);
        raw["event"]["message"]["parent_id"] = "om_other".into();
        let result = setup.launch(raw.clone()).await;
        if enabled {
            assert!(matches!(result,LaunchOutcome::Refused(p) if p.status==409));
        } else {
            admitted(result);
        }
        assert_eq!(serde_json::to_vec(&setup.plan(&raw).await).unwrap(), before);
        for target in &original.targets {
            assert!(target.launch.event.verifies_activity(&original.activity));
        }
    }
}
