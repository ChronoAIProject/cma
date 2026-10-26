use super::{
    dispatch_fixture::Setup,
    inbox_dispatch::InboxDispatcher,
    inbox_scan::{Disposition, InboxStore},
};
use cmaeg_core::channel::Secret;
use std::collections::BTreeSet;

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
