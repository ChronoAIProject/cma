#[path = "restore/routed.rs"]
mod routed;
#[path = "../../cmaeg-core/tests/routing_policy/fixture.rs"]
mod routing_fixture;
#[path = "restore/support.rs"]
mod support;
use cmaeg::{channels::resolver::ResolveError, http::readiness::Readiness, ChannelCacheConfig};
use futures_util::future::join_all;
use std::{
    sync::{atomic::Ordering, Arc},
    time::{Duration, Instant},
};
use support::*;

#[tokio::test]
async fn concurrent_cold_calls_share_one_get_and_warm_hits_share_the_arc() {
    let mut fixture = Fixture::new(ChannelCacheConfig::default()).await;
    fixture.server.state.delay_ms.store(50, Ordering::SeqCst);
    let results = join_all((0..50).map(|_| fixture.resolver.resolve(fixture.id.as_str()))).await;
    assert_eq!(fixture.server.count(), 1);
    let first = results[0].as_ref().unwrap();
    assert_eq!(first.version, 1);
    assert_eq!(first.agent_key_hash, [0; 32]);
    // A legacy channel's only consumer is the default, decoded from the same hash.
    assert_eq!(
        first.consumer_keys,
        [(cmaeg_core::ids::ConsumerId::default_consumer(), [0; 32])]
    );
    for result in &results {
        assert!(Arc::ptr_eq(first, result.as_ref().unwrap()));
    }
    let warm = fixture.resolver.resolve(fixture.id.as_str()).await.unwrap();
    assert!(Arc::ptr_eq(first, &warm));
    assert_eq!(fixture.server.count(), 1);
    fixture.server.stop().await;
}

#[tokio::test]
async fn negative_entries_expire_and_unavailable_restores_are_rate_limited_per_id() {
    let config = ChannelCacheConfig {
        negative_ttl: Duration::from_millis(30),
        ..Default::default()
    };
    let mut fixture = Fixture::new(config).await;
    *fixture.server.state.record.lock().unwrap() = None;
    for _ in 0..2 {
        assert_eq!(
            fixture
                .resolver
                .resolve(fixture.id.as_str())
                .await
                .unwrap_err(),
            ResolveError::NotFound
        );
    }
    assert_eq!(fixture.server.count(), 1);
    tokio::time::sleep(Duration::from_millis(40)).await;
    assert_eq!(
        fixture
            .resolver
            .resolve(fixture.id.as_str())
            .await
            .unwrap_err(),
        ResolveError::NotFound
    );
    assert_eq!(fixture.server.count(), 2);
    fixture.server.stop().await;

    let mut fixture = Fixture::new(ChannelCacheConfig::default()).await;
    fixture.server.state.fail.store(true, Ordering::SeqCst);
    for _ in 0..3 {
        assert_eq!(
            fixture
                .resolver
                .resolve(fixture.id.as_str())
                .await
                .unwrap_err(),
            ResolveError::Unavailable
        );
        assert_eq!(fixture.resolver.cache_len(), 0);
    }
    assert_eq!(
        fixture
            .resolver
            .resolve(fixture.id.as_str())
            .await
            .unwrap_err(),
        ResolveError::RateLimited
    );
    assert_eq!(fixture.server.count(), 3);
    let mut pointer = fixture.id.open(&fixture.keys).unwrap().payload;
    pointer.record_id = uuid::Uuid::new_v4().to_string().parse().unwrap();
    pointer.record_url = fixture
        .records
        .record_url(pointer.record_id.as_str())
        .unwrap()
        .to_string();
    let other = pointer.seal_id(&fixture.keys).unwrap();
    assert_eq!(
        fixture.resolver.resolve(other.as_str()).await.unwrap_err(),
        ResolveError::Unavailable
    );
    assert_eq!(fixture.server.count(), 4);
    fixture.server.stop().await;
}

#[tokio::test]
async fn malformed_and_tampered_ids_never_reach_cma() {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    let mut fixture = Fixture::new(ChannelCacheConfig::default()).await;
    assert_eq!(
        fixture
            .resolver
            .resolve("cmaeg1.ch.invalid")
            .await
            .unwrap_err(),
        ResolveError::Invalid
    );
    let mut parts: Vec<_> = fixture.id.as_str().split('.').map(str::to_owned).collect();
    let mut bytes = URL_SAFE_NO_PAD.decode(&parts[3]).unwrap();
    bytes[12] ^= 1;
    parts[3] = URL_SAFE_NO_PAD.encode(bytes);
    assert_eq!(
        fixture
            .resolver
            .resolve(&parts.join("."))
            .await
            .unwrap_err(),
        ResolveError::Tampered
    );
    assert_eq!(fixture.server.count(), 0);
    fixture.server.stop().await;
}

#[tokio::test]
async fn stale_reads_refresh_in_background_and_stop_after_one_extra_ttl() {
    let config = ChannelCacheConfig {
        ttl: Duration::from_millis(100),
        ..Default::default()
    };
    let mut fixture = Fixture::new(config).await;
    let old = fixture.resolver.resolve(fixture.id.as_str()).await.unwrap();
    tokio::time::sleep(Duration::from_millis(110)).await;
    fixture.store(2);
    fixture.server.state.delay_ms.store(40, Ordering::SeqCst);
    let stale = fixture.resolver.resolve(fixture.id.as_str()).await.unwrap();
    assert!(Arc::ptr_eq(&old, &stale));
    fixture.server.wait_for_gets(2).await;
    // A request reaching the server is not proof that the background restore
    // has published. Observe publication within the restore budget, including
    // on a busy test runner, instead of racing a fixed scheduling delay.
    let fresh = tokio::time::timeout(Duration::from_millis(800), async {
        loop {
            let current = fixture.resolver.resolve(fixture.id.as_str()).await.unwrap();
            if current.version == 2 {
                break current;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(fresh.version, 2);
    assert_eq!(fixture.server.count(), 2);
    tokio::time::sleep(Duration::from_millis(220)).await;
    fixture.server.state.fail.store(true, Ordering::SeqCst);
    assert_eq!(
        fixture
            .resolver
            .resolve(fixture.id.as_str())
            .await
            .unwrap_err(),
        ResolveError::Unavailable
    );
    fixture.server.stop().await;
}

#[tokio::test]
async fn invalidation_during_restore_cannot_publish_an_old_record() {
    let mut fixture = Fixture::new(ChannelCacheConfig::default()).await;
    fixture.server.state.delay_ms.store(40, Ordering::SeqCst);
    let resolver = fixture.resolver.clone();
    let id = fixture.id.to_string();
    let pending = tokio::spawn(async move { resolver.resolve(&id).await });
    fixture.server.wait_for_gets(1).await;
    fixture.store(2);
    fixture.resolver.invalidate(fixture.id.as_str());
    let result = pending.await.unwrap().unwrap();
    assert_eq!(result.version, 2);
    assert_eq!(fixture.server.count(), 2);
    assert_eq!(
        fixture
            .resolver
            .resolve(fixture.id.as_str())
            .await
            .unwrap()
            .version,
        2
    );
    fixture.server.stop().await;
}

#[tokio::test]
async fn cancelled_leader_does_not_strand_other_waiters() {
    let mut fixture = Fixture::new(ChannelCacheConfig::default()).await;
    fixture.server.state.delay_ms.store(30, Ordering::SeqCst);
    let resolver = fixture.resolver.clone();
    let id = fixture.id.to_string();
    let pending = tokio::spawn(async move { resolver.resolve(&id).await });
    fixture.server.wait_for_gets(1).await;
    pending.abort();
    assert!(pending.await.unwrap_err().is_cancelled());
    tokio::time::timeout(
        Duration::from_secs(1),
        fixture.resolver.resolve(fixture.id.as_str()),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(fixture.server.count(), 1);
    fixture.server.stop().await;
}

#[tokio::test]
async fn restore_budget_and_cma_reachability_are_independent_of_readiness() {
    let mut fixture = Fixture::new(ChannelCacheConfig::default()).await;
    fixture.server.state.delay_ms.store(300, Ordering::SeqCst);
    let started = Instant::now();
    fixture.resolver.resolve(fixture.id.as_str()).await.unwrap();
    assert!(started.elapsed() < Duration::from_millis(400));
    let ready = Readiness::initialized();
    ready.probe_once(&fixture.records).await;
    assert!(ready.report().ready && ready.report().cma_reachable);
    fixture.server.stop().await;
    ready.probe_once(&fixture.records).await;
    assert!(ready.report().ready && !ready.report().cma_reachable);
    fixture.server.restart().await;
    ready.probe_once(&fixture.records).await;
    assert!(ready.report().ready && ready.report().cma_reachable);
    fixture.server.stop().await;
}

#[tokio::test]
async fn slow_restore_times_out_without_negative_caching() {
    let config = ChannelCacheConfig {
        restore_timeout: Duration::from_millis(30),
        ..Default::default()
    };
    let mut fixture = Fixture::new(config).await;
    fixture.server.state.delay_ms.store(100, Ordering::SeqCst);
    assert_eq!(
        fixture
            .resolver
            .resolve(fixture.id.as_str())
            .await
            .unwrap_err(),
        ResolveError::Unavailable
    );
    assert_eq!(fixture.resolver.cache_len(), 0);
    fixture.server.state.delay_ms.store(0, Ordering::SeqCst);
    fixture.resolver.resolve(fixture.id.as_str()).await.unwrap();
    assert_eq!(fixture.server.count(), 2);
    fixture.server.stop().await;
}

#[path = "restore/http.rs"]
mod http;

#[tokio::test]
async fn cache_capacity_evicts_old_entries_and_invalid_blobs_never_become_warm() {
    let mut fixture = Fixture::new(ChannelCacheConfig {
        max_entries: 3,
        ..Default::default()
    })
    .await;
    *fixture.server.state.record.lock().unwrap() = None;
    let mut ids = Vec::new();
    for _ in 0..4 {
        let mut pointer = fixture.id.open(&fixture.keys).unwrap().payload;
        pointer.record_id = uuid::Uuid::new_v4().to_string().parse().unwrap();
        pointer.record_url = fixture
            .records
            .record_url(pointer.record_id.as_str())
            .unwrap()
            .to_string();
        let id = pointer.seal_id(&fixture.keys).unwrap();
        assert_eq!(
            fixture.resolver.resolve(id.as_str()).await.unwrap_err(),
            ResolveError::NotFound
        );
        assert!(fixture.resolver.cache_len() <= 3);
        ids.push(id);
    }
    assert_eq!(fixture.server.count(), 4);
    assert_eq!(
        fixture.resolver.resolve(ids[0].as_str()).await.unwrap_err(),
        ResolveError::NotFound
    );
    assert_eq!(fixture.server.count(), 5);
    fixture.server.stop().await;

    let mut fixture = Fixture::new(ChannelCacheConfig::default()).await;
    *fixture.server.state.record.lock().unwrap() = Some((b"broken".to_vec(), 1));
    assert_eq!(
        fixture
            .resolver
            .resolve(fixture.id.as_str())
            .await
            .unwrap_err(),
        ResolveError::InvalidDefinition
    );
    assert_eq!(fixture.resolver.cache_len(), 0);
    let original = fixture.definition.record_id.clone();
    fixture.definition.record_id = uuid::Uuid::new_v4().to_string().parse().unwrap();
    fixture.store(2);
    assert_eq!(
        fixture
            .resolver
            .resolve(fixture.id.as_str())
            .await
            .unwrap_err(),
        ResolveError::InvalidDefinition
    );
    assert_eq!(fixture.resolver.cache_len(), 0);
    fixture.definition.record_id = original;
    fixture.store(3);
    assert_eq!(
        fixture
            .resolver
            .resolve(fixture.id.as_str())
            .await
            .unwrap()
            .version,
        3
    );
    fixture.server.stop().await;
}
