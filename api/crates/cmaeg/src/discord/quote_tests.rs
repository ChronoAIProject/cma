use super::{
    journal_fixture::Fixture,
    normalization_tests::{channel, raw},
    Dispatch, InboxLaunch, InboxReader,
};
use crate::connections::store::index_id;
use cmaeg_core::{activity::ActivityKind, seal::ReplyAddress};
use serde_json::json;
#[tokio::test]
async fn discord_quote_projection_is_fixed_at_inbox_capture_and_replay_cannot_upgrade_it() {
    for enabled in [false, true] {
        let mut f = Fixture::new().await;
        f.api.quoted_context = enabled;
        let journal = f.acquire().await.unwrap();
        let mut event = raw();
        event["id"] = f.message_id().into();
        event["type"] = 19.into();
        event["message_reference"] = json!({"type":0,"message_id":"600","channel_id":"99"});
        event["referenced_message"] = json!({"id":"600","channel_id":"99","author":{"id":"55","bot":false},"content":"quoted passage","attachments":[]});
        let mut dispatch = Dispatch {
            name: "MESSAGE_CREATE".into(),
            sequence: 1,
            data: event,
        };
        journal
            .persist_message(&journal.authority, &dispatch)
            .await
            .unwrap();
        let id = index_id(
            "discord-native-message-v1",
            &[&f.record, "77", dispatch.data["id"].as_str().unwrap()],
        );
        let reader = InboxReader::new(f.store.channels.clone(), f.store.keys.clone());
        let now = (f.store.clock)() / 1000;
        let InboxLaunch::Launch(first) = reader.load(&id, channel(), now).await.unwrap() else {
            panic!()
        };
        assert_eq!(first.quote_projection.is_some(), enabled);
        assert!(
            matches!(&first.request.activity.kind,ActivityKind::Message{quoted_message,..} if quoted_message.is_some()==enabled)
        );
        let event_ref = first
            .request
            .event_ref
            .open(&f.store.keys, now)
            .unwrap()
            .payload;
        assert!(event_ref.verifies_activity(&first.request.activity));
        assert!(
            matches!(&event_ref.reply,ReplyAddress::Discord{message_id,..} if message_id==dispatch.data["id"].as_str().unwrap())
        );
        let original_bytes = first.source_bytes.clone();
        let original_activity = serde_json::to_vec(&first.request.activity).unwrap();
        journal.release().await.unwrap();
        drop(journal);
        f.api.quoted_context = !enabled;
        let journal = f.acquire().await.unwrap();
        journal
            .persist_message(&journal.authority, &dispatch)
            .await
            .unwrap();
        let InboxLaunch::Launch(replay) = reader.load(&id, channel(), now).await.unwrap() else {
            panic!()
        };
        assert_eq!(replay.source_bytes, original_bytes);
        assert_eq!(
            serde_json::to_vec(&replay.request.activity).unwrap(),
            original_activity
        );
        dispatch.data["referenced_message"]["content"] = "changed quote".into();
        let result = journal.persist_message(&journal.authority, &dispatch).await;
        assert_eq!(result.is_err(), enabled);
        let InboxLaunch::Launch(replay) = reader.load(&id, channel(), now).await.unwrap() else {
            panic!()
        };
        assert_eq!(replay.source_bytes, original_bytes);
    }
}
