use super::{
    dispatch_fixture::Setup,
    inbox_dispatch::InboxDispatcher,
    inbox_scan::{Disposition, InboxStore},
};
use cmaeg_core::channel::Secret;
use std::collections::BTreeSet;

#[tokio::test]
async fn delayed_native_admission_uses_the_same_clock_as_its_record_service() {
    let f = Setup::new(true).await;
    // Wall time advances while the fixture's durable source clock stays fixed.
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    f.message(|_| {}).await;
    let page = f.sweep(f.now()).await;
    assert_eq!(page.pending, 0);
    assert_eq!(page.acknowledged, 1);
    let plans = f.plans.recover_all_page(None, 32).await.unwrap().plans;
    assert_eq!(plans.len(), 1);
    assert_eq!(plans[0].admitted_at, f.now());
    assert_eq!(plans[0].retained_until, f.now() + 7 * 86400);
    assert_eq!(f.direct_calls(), 0);

    // Explicit fixture time travel must advance both admission clocks as well.
    f.fixture.advance(60_000);
    f.message(|_| {}).await;
    let page = f.sweep(f.now()).await;
    assert_eq!(page.pending, 0);
    assert_eq!(page.acknowledged, 2);
    let plans = f.plans.recover_all_page(None, 32).await.unwrap().plans;
    assert_eq!(plans.len(), 2);
    assert!(plans
        .iter()
        .any(|plan| plan.admitted_at == f.now() && plan.retained_until == f.now() + 7 * 86400));
    assert_eq!(f.direct_calls(), 0);
}

#[tokio::test]
async fn native_inbox_commits_one_plan_and_reuses_it_after_lost_ack_response() {
    let f = Setup::new(true).await;
    f.message(|_| {}).await;
    let store = InboxStore::new(
        f.fixture.store.channels.clone(),
        f.fixture.store.keys.clone(),
    );
    let page = store.page(None, f.now()).await.unwrap();
    let entry = &page.entries[0];
    f.fixture.fail(&entry.ack_id(), true);
    assert_eq!(f.sweep(f.now()).await.acknowledged, 1);
    assert!(store.settled(entry).await.unwrap());
    assert_eq!(f.sweep(f.now()).await.acknowledged, 1);
    let plans = f.plans.recover_all_page(None, 32).await.unwrap().plans;
    assert_eq!(plans.len(), 1);
    assert!(String::from_utf8_lossy(&plans[0].source_body).contains("dispatch"));
    assert_eq!(f.direct_calls(), 0);
}

#[tokio::test]
async fn absent_ack_retry_restores_durable_plan_and_exact_original_payload() {
    let f = Setup::new(true).await;
    f.message(|_| {}).await;
    let store = InboxStore::new(
        f.fixture.store.channels.clone(),
        f.fixture.store.keys.clone(),
    );
    let page = store.page(None, f.now()).await.unwrap();
    let entry = &page.entries[0];
    f.fixture.fail(&entry.ack_id(), false);
    assert_eq!(f.sweep(f.now()).await.pending, 1);
    assert!(!store.settled(entry).await.unwrap());
    let first = f
        .plans
        .recover_all_page(None, 32)
        .await
        .unwrap()
        .plans
        .pop()
        .unwrap();
    assert_eq!(f.sweep(f.now()).await.acknowledged, 1);
    let plans = f.plans.recover_all_page(None, 32).await.unwrap().plans;
    assert_eq!(plans.len(), 1);
    assert_eq!(plans[0].source_body, first.source_body);
    assert_eq!(plans[0].admission_id, first.admission_id);
    assert_eq!(f.direct_calls(), 0);
}

#[tokio::test]
async fn missing_routing_and_oversized_payload_are_durably_refused_without_provider_effects() {
    for (routed, large) in [(false, false), (true, true)] {
        let f = Setup::new(routed).await;
        f.message(|event| {
            if large {
                event["unused_native_field"] = "x".repeat(300_000).into();
            }
        })
        .await;
        assert_eq!(f.sweep(f.now()).await.acknowledged, 1);
        assert!(f
            .plans
            .recover_all_page(None, 32)
            .await
            .unwrap()
            .plans
            .is_empty());
        assert_eq!(f.direct_calls(), 0);
    }
}

#[tokio::test]
async fn revoked_source_never_admits_and_expiry_never_renews_authority() {
    let f = Setup::new(true).await;
    f.message(|_| {}).await;
    let mut revoked = f.channel.clone();
    revoked.secrets.discord_bot_token = Some(Secret::new("rotated-native-fixture"));
    f.fixture.replace(&revoked);
    assert_eq!(f.sweep(f.now()).await.pending, 1);
    assert!(f
        .plans
        .recover_all_page(None, 32)
        .await
        .unwrap()
        .plans
        .is_empty());
    f.fixture.replace(&f.channel);
    assert_eq!(f.sweep(f.now() + 1800).await.acknowledged, 1);
    assert!(f
        .plans
        .recover_all_page(None, 32)
        .await
        .unwrap()
        .plans
        .is_empty());
    assert_eq!(f.direct_calls(), 0);
}

#[tokio::test]
async fn native_admission_refusals_and_empty_selection_never_touch_a_provider() {
    let f = Setup::new(true).await;
    f.message(|event| event["author"]["bot"] = true.into())
        .await;
    assert_eq!(f.sweep(f.now()).await.acknowledged, 1);
    let store = InboxStore::new(
        f.fixture.store.channels.clone(),
        f.fixture.store.keys.clone(),
    );
    let page = store.page(None, f.now()).await.unwrap();
    assert!(store
        .acknowledge(&page.entries[0], Disposition::Expired)
        .await
        .is_err());
    let empty = InboxDispatcher::new(
        crate::records::RecordClient::new("http://127.0.0.1:1", "no-network-fixture").unwrap(),
        f.fixture.store.keys.clone(),
        f.state.clone(),
        BTreeSet::new(),
    );
    assert_eq!(empty.page(None, f.now()).await.unwrap().acknowledged, 0);
    assert_eq!(f.direct_calls(), 0);
}

#[tokio::test]
async fn discovered_native_selection_admits_only_verified_inbox_and_receipt_precedes_execution() {
    let mut f = Setup::discovered().await;
    let activity = f.observe();
    let id = f.message(|_| {}).await;
    let current = super::runtime_source::resolve(
        &f.fixture.store.channels,
        &f.fixture.store.keys,
        &f.state.resolver,
        &f.fixture.record,
    )
    .await
    .unwrap();
    let super::InboxLaunch::Launch(verified) = super::InboxReader::new(
        f.fixture.store.channels.clone(),
        f.fixture.store.keys.clone(),
    )
    .load(&id, current.channel, f.now())
    .await
    .unwrap() else {
        panic!("expected admitted native message")
    };
    assert!(!f
        .state
        .routed
        .as_ref()
        .unwrap()
        .selected(&f.channel.record_id));
    assert!(f
        .state
        .capture_routed(&verified.request, b"arbitrary HTTP bytes", true)
        .is_none());
    assert_eq!(
        activity
            .latest(&f.channel.owner.subject, &f.fixture.record)
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    f.selection.remove(&f.fixture.record);
    assert_eq!(f.sweep(f.now()).await.acknowledged, 0);
    assert!(f
        .plans
        .recover_all_page(None, 32)
        .await
        .unwrap()
        .plans
        .is_empty());
    assert!(f.selection.insert(f.fixture.record.clone()));
    assert_eq!(f.sweep(f.now()).await.acknowledged, 1);
    assert_eq!(
        f.plans
            .recover_all_page(None, 32)
            .await
            .unwrap()
            .plans
            .len(),
        1
    );
    assert_eq!(f.direct_calls(), 0);
}
