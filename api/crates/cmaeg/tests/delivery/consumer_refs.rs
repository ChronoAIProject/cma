//! A message ref minted for a relay or native Telegram reply names the
//! consumer its event was issued to, and nothing for the default consumer,
//! whose refs keep their exact bytes (I3).
use super::{
    fake_http::FakeHttp,
    recording_fixtures::{
        activity, event, keys, native_channel, relay_activity, relay_channel, relay_reply,
    },
    recording_support::{Hooks, Recording},
};
use cmaeg::{delivery::intents::ReplySource, DeliveryConfig};
use cmaeg_core::{activity::SourceKind, channel::Platform, ids::ConsumerId, seal::ReplyAddress};
use serde_json::json;

#[tokio::test]
async fn reply_message_refs_name_the_events_consumer() {
    let native = SourceKind::Telegram {
        platform: Platform::Telegram,
        bot_id: "77".into(),
    };
    let native_reply = ReplyAddress::Telegram {
        bot_id: "77".into(),
        chat_id: "42".into(),
        message_id: "5".into(),
        thread_id: None,
    };
    let paths = [
        ("relay", relay_channel(0), relay_activity(), relay_reply()),
        (
            "native",
            native_channel(),
            activity(native, "msg-2"),
            native_reply,
        ),
    ];
    for (path, channel, activity, reply) in paths {
        for consumer in [None, Some(ConsumerId::parse("csm_00000000000a").unwrap())] {
            let mut recording = Recording::start_with(
                FakeHttp::platforms(),
                Hooks::answering([]),
                DeliveryConfig::default(),
                None,
                false,
            );
            let mut event = event(&channel, &activity, 1, reply.clone());
            event.consumer = consumer.clone();
            let receipt = recording
                .reply(
                    &channel,
                    &event,
                    json!({"text": "Done."}),
                    ReplySource::Tool,
                )
                .await;
            let reference = receipt.message_ref.expect("a confirmed reply has a ref");
            let payload = reference.open(&keys()).unwrap().payload;
            assert_eq!(payload.consumer, consumer, "{path}");
            recording.finish().await;
        }
    }
}
