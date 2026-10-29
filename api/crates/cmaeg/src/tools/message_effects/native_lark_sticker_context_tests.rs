use super::native_lark_fixture::Fixture;
use crate::native_lark::{self, message, test_support};
use cmaeg_core::{
    channel::{LarkRegion, Source},
    seal::EventRef,
};
use serde_json::json;

#[tokio::test]
async fn received_sticker_capability_needs_exact_retention_gate_and_current_caller() {
    for named in [false, true] {
        let mut f = Fixture::new(LarkRegion::Feishu, named, false).await;
        let Source::NativeLark(source) = &f.app.definition.sources[0] else {
            panic!()
        };
        let now = time::OffsetDateTime::now_utc().unix_timestamp();
        let mut raw = test_support::text("om_context_sticker", "", now);
        raw["event"]["message"]["message_type"] = "sticker".into();
        raw["event"]["message"]["content"] =
            json!({"file_key":"context-private-key"}).to_string().into();
        let received = message::extract(source, &raw, now).unwrap().unwrap();
        let channel = f
            .app
            .state
            .resolver
            .resolve_current(f.app.event.channel_id.as_str())
            .await
            .unwrap();
        let request = message::launch(
            channel,
            source,
            &received,
            None,
            &f.app.state.keys,
            std::time::Instant::now(),
            now,
        )
        .unwrap();
        (f.app.activity, f.app.event) =
            crate::turns::routed::bind_received_message_fixture_with_source(
                f.app.state.resolver.record_client(),
                f.app.state.keys.clone(),
                &request,
                f.app.event.consumer.as_ref(),
                &serde_json::to_vec(&received).unwrap(),
            )
            .await;
        assert!(f
            .context()
            .await
            .capabilities
            .reply_with_received_sticker
            .is_none());
        f.app.state.native_lark_sticker_replies_enabled = true;
        let context = f.context().await;
        assert_eq!(context.capabilities.reply_with_received_sticker, Some(true));
        assert!(!serde_json::to_string(&context)
            .unwrap()
            .contains("context-private-key"));
        let reference = EventRef::seal(&f.app.event, &f.app.state.keys).unwrap();
        assert!(f
            .app
            .state
            .verify_context(
                crate::auth::AgentCaller { key_hash: [77; 32] },
                &reference,
                f.app.activity.clone()
            )
            .await
            .is_err());
        f.app.definition.sources.clear();
        f.app.replace();
        let resolved = f
            .app
            .state
            .resolver
            .resolve_current(f.app.event.channel_id.as_str())
            .await;
        // A removed source may invalidate the whole current definition.
        if let Ok(channel) = resolved {
            assert!(native_lark::stickers::load(
                f.app.state.resolver.record_client(),
                f.app.state.keys.clone(),
                &channel,
                &f.app.event
            )
            .await
            .is_none());
        }
    }
}
