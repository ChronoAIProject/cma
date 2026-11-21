use super::{
    dispatch_fixture::Setup, journal_fixture::Fixture, journal_model::Inbox,
    normalization_tests::raw, protocol::checkpoint, Dispatch, GatewayError, NativeDiscordJournal,
};
use crate::{
    activity::{ActivityStore, ObservedActivity},
    connections::store::index_id,
    turns::observer::{ActivityObserver, NoopObserver},
};
use cmaeg_core::{activity::Activity, ids::RecordId};
use futures_util::future::BoxFuture;
use serde_json::json;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

fn timestamp(seconds: i64) -> String {
    time::OffsetDateTime::from_unix_timestamp(seconds)
        .unwrap()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap()
}

#[tokio::test]
async fn native_receipt_is_visible_before_any_scan_or_launch_capacity() {
    let mut f = Setup::new(true).await;
    let activity = f.observe();
    let mut updates = activity.subscribe();
    let _capacity = f
        .state
        .inflight
        .clone()
        .acquire_many_owned(f.state.inflight.available_permits() as u32)
        .await
        .unwrap();
    let received_at = f.now();
    f.message(|_| {}).await;
    assert_eq!(updates.try_recv().unwrap(), f.fixture.record);
    let page = activity
        .latest(&f.channel.owner.subject, &f.fixture.record)
        .await
        .unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].received_at, timestamp(received_at));
    assert_eq!(
        page.items[0].occurred_at,
        raw()["timestamp"].as_str().unwrap()
    );
    assert!(page.items[0].consumers.is_empty());
    assert!(f
        .plans
        .recover_all_page(None, 32)
        .await
        .unwrap()
        .plans
        .is_empty());
    assert_eq!(f.direct_calls(), 0);
    assert_eq!(f.sweep(f.now()).await.pending, 1);
    let retained = activity
        .latest(&f.channel.owner.subject, &f.fixture.record)
        .await
        .unwrap();
    assert_eq!(retained.items[0].received_at, page.items[0].received_at);
}

#[tokio::test]
async fn native_recovery_restores_first_receipt_before_capacity_and_never_renews_it() {
    let mut f = Setup::new(true).await;
    f.message(|_| {}).await;
    let received_at = f.now();
    // The inbox survived a process loss before its Activity write. The next
    // reader has no cached Activity context and cannot acquire launch capacity.
    f.fixture.advance(60_000);
    let activity = f.observe();
    let _capacity = f
        .state
        .inflight
        .clone()
        .acquire_many_owned(f.state.inflight.available_permits() as u32)
        .await
        .unwrap();
    assert_eq!(f.sweep(f.now()).await.pending, 1);
    let first = activity
        .latest(&f.channel.owner.subject, &f.fixture.record)
        .await
        .unwrap();
    assert_eq!(first.items.len(), 1);
    assert_eq!(first.items[0].received_at, timestamp(received_at));
    f.fixture.advance(60_000);
    let restarted = f.observe();
    assert_eq!(f.sweep(f.now()).await.pending, 1);
    let replay = restarted
        .latest(&f.channel.owner.subject, &f.fixture.record)
        .await
        .unwrap();
    assert_eq!(replay.items.len(), 1);
    assert_eq!(replay.items[0].received_at, first.items[0].received_at);
    assert_eq!(replay.items[0].occurred_at, first.items[0].occurred_at);
    assert!(f
        .plans
        .recover_all_page(None, 32)
        .await
        .unwrap()
        .plans
        .is_empty());
    assert_eq!(f.direct_calls(), 0);
}

struct FailableActivity {
    unavailable: AtomicBool,
    activity: ObservedActivity,
}

#[tokio::test]
async fn expired_native_recovery_records_receipt_without_renewing_reply_authority() {
    let mut f = Setup::new(true).await;
    f.message(|_| {}).await;
    let received_at = f.now();
    f.fixture.advance(1_800_000);
    let activity = f.observe();
    assert_eq!(f.sweep(f.now()).await.acknowledged, 1);
    let page = activity
        .latest(&f.channel.owner.subject, &f.fixture.record)
        .await
        .unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].received_at, timestamp(received_at));
    assert!(page.items[0].consumers.is_empty());
    assert!(f
        .plans
        .recover_all_page(None, 32)
        .await
        .unwrap()
        .plans
        .is_empty());
    assert_eq!(f.direct_calls(), 0);
}
impl ActivityObserver for FailableActivity {
    fn receive_at<'a>(
        &'a self,
        owner: &'a str,
        record: &'a RecordId,
        activity: &'a Activity,
        received_at: i64,
    ) -> BoxFuture<'a, Result<(), ()>> {
        Box::pin(async move {
            if self.unavailable.load(Ordering::SeqCst) {
                return Err(());
            }
            self.activity
                .receive_at(owner, record, activity, received_at)
                .await
        })
    }
}

#[tokio::test]
async fn refused_native_messages_do_not_wait_for_activity_or_poison_the_checkpoint() {
    let f = Fixture::new().await;
    let activity = Arc::new(ActivityStore::new(
        f.store.channels.clone(),
        f.store.keys.clone(),
    ));
    let observer = Arc::new(FailableActivity {
        unavailable: AtomicBool::new(true),
        activity: ObservedActivity::new(activity.clone(), Arc::new(NoopObserver)),
    });
    let journal = f.acquire_observed(observer).await.unwrap();
    journal.authorize_connection(None).await.unwrap();
    let ready = Dispatch {
        name: "READY".into(),
        sequence: 0,
        data: json!({"session_id":"refused-activity-fixture","resume_gateway_url":"wss://gateway.discord.gg/","user":{"id":"77","bot":true}}),
    };
    let mut saved = checkpoint(None, &ready, "77").unwrap();
    journal.admit(&saved, &ready).await.unwrap();
    for index in 0..4 {
        f.advance(1_000);
        let mut data = raw();
        data["id"] = f.message_id().into();
        match index {
            0 => data["author"]["bot"] = true.into(),
            1 => data["edited_timestamp"] = "2026-10-26T00:00:01Z".into(),
            2 => data["type"] = 7.into(),
            _ => data["content"] = serde_json::Value::Null,
        }
        saved.sequence = index + 1;
        let message = Dispatch {
            name: "MESSAGE_CREATE".into(),
            sequence: saved.sequence,
            data,
        };
        journal.admit(&saved, &message).await.unwrap();
        assert_eq!(
            journal.checkpoint().await.unwrap().unwrap().sequence,
            saved.sequence
        );
    }
    assert!(activity
        .latest(&journal.authority.owner, &f.record)
        .await
        .unwrap()
        .items
        .is_empty());
}

#[tokio::test]
async fn native_checkpoint_waits_for_activity_and_replay_keeps_the_winning_receipt() {
    let f = Fixture::new().await;
    let activity = Arc::new(ActivityStore::new(
        f.store.channels.clone(),
        f.store.keys.clone(),
    ));
    let observer = Arc::new(FailableActivity {
        unavailable: AtomicBool::new(true),
        activity: ObservedActivity::new(activity.clone(), Arc::new(NoopObserver)),
    });
    let journal = f.acquire_observed(observer.clone()).await.unwrap();
    journal.authorize_connection(None).await.unwrap();
    let ready = Dispatch {
        name: "READY".into(),
        sequence: 0,
        data: json!({"session_id":"activity-fixture","resume_gateway_url":"wss://gateway.discord.gg/","user":{"id":"77","bot":true}}),
    };
    let mut saved = checkpoint(None, &ready, "77").unwrap();
    journal.admit(&saved, &ready).await.unwrap();
    let mut data = raw();
    data["id"] = f.message_id().into();
    let mut message = Dispatch {
        name: "MESSAGE_CREATE".into(),
        sequence: 1,
        data,
    };
    saved.sequence = 1;
    let received_at = (f.store.clock)() / 1000;
    let id = index_id(
        "discord-native-message-v1",
        &[&f.record, "77", message.message_id().unwrap()],
    );
    f.fail(&id, false);
    assert_eq!(
        journal.admit(&saved, &message).await,
        Err(GatewayError::Unavailable)
    );
    assert!(f.store.load::<Inbox>(&id, true).await.unwrap().is_none());
    assert!(activity
        .latest(&journal.authority.owner, &f.record)
        .await
        .unwrap()
        .items
        .is_empty());
    assert_eq!(
        journal.admit(&saved, &message).await,
        Err(GatewayError::Unavailable)
    );
    assert!(f.store.load::<Inbox>(&id, true).await.unwrap().is_some());
    assert_eq!(journal.checkpoint().await.unwrap().unwrap().sequence, 0);
    assert!(activity
        .latest(&journal.authority.owner, &f.record)
        .await
        .unwrap()
        .items
        .is_empty());
    observer.unavailable.store(false, Ordering::SeqCst);
    f.advance(1_000);
    message.data["author"]["username"] = "changed redelivery".into();
    journal.admit(&saved, &message).await.unwrap();
    assert_eq!(journal.checkpoint().await.unwrap().unwrap().sequence, 1);
    let first = activity
        .latest(&journal.authority.owner, &f.record)
        .await
        .unwrap();
    assert_eq!(first.items.len(), 1);
    assert_eq!(first.items[0].received_at, timestamp(received_at));
    assert_eq!(first.items[0].actor.display_name.as_deref(), Some("human"));
    f.advance(1_000);
    saved.sequence = 2;
    message.sequence = 2;
    journal.admit(&saved, &message).await.unwrap();
    let replay = activity
        .latest(&journal.authority.owner, &f.record)
        .await
        .unwrap();
    assert_eq!(replay.items.len(), 1);
    assert_eq!(replay.items[0].received_at, first.items[0].received_at);
    // A changed source credential still fences intake before Activity writes.
    let mut revoked = super::journal_fixture::definition();
    revoked.secrets.discord_bot_token = Some(cmaeg_core::channel::Secret::new(
        "rotated-native-activity-fixture",
    ));
    f.replace(&revoked);
    saved.sequence = 3;
    message.sequence = 3;
    message.data["id"] = f.message_id().into();
    assert_eq!(
        journal.admit(&saved, &message).await,
        Err(GatewayError::Authority)
    );
    assert_eq!(
        activity
            .latest(&journal.authority.owner, &f.record)
            .await
            .unwrap()
            .items
            .len(),
        1
    );
}
