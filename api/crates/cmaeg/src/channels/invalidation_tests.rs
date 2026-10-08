//! The I14 mutation scan budget, with complete real cache entries but no I/O.
use super::*;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use std::time::Duration;

fn seed(
    resolver: &ChannelResolver,
    pointer: &ChannelPointer,
    definition: &ChannelDefinition,
) -> String {
    let id = pointer.seal_id(&resolver.inner.keys).unwrap();
    let raw_id = id.to_string();
    let channel = Arc::new(ResolvedChannel {
        id,
        pointer: pointer.clone(),
        definition: definition.clone(),
        agent_key_hash: [0; 32],
        consumer_keys: vec![(ConsumerId::default_consumer(), [0; 32])],
        loaded_at: Instant::now(),
        version: 1,
    });
    resolver.inner.cache.insert(
        raw_id.clone(),
        CacheEntry::Present(channel, Instant::now() + resolver.inner.config.ttl),
    );
    raw_id
}

#[test]
#[ignore = "release-mode budget for one match among 50,000 cached aliases, no restore flights"]
fn invalidate_record_latency_budget() {
    let keys = Arc::new(MasterKeyRing::parse(&format!("1:{}", STANDARD.encode([7; 32]))).unwrap());
    let base = "http://127.0.0.1:8090";
    let config = ChannelCacheConfig {
        max_entries: 50_000,
        ttl: Duration::from_secs(3600),
        ..Default::default()
    };
    let resolver = ChannelResolver::new(
        Arc::new(RecordClient::new(base, "cma-service").unwrap()),
        keys,
        config,
    );
    let definition: ChannelDefinition = serde_json::from_str(include_str!(
        "../../../cmaeg-core/tests/fixtures/channel.json"
    ))
    .unwrap();
    let mut definition = ChannelDefinition {
        profile_digest: definition.consumer(None).unwrap().profile.digest(),
        record_id: RecordId::new(uuid::Uuid::new_v4().to_string()).unwrap(),
        ..definition
    };
    definition.validate().unwrap();
    let target = super::super::definition::channel_pointer(base, &definition.record_id).unwrap();
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_writer(std::io::sink)
        .finish();
    let _subscriber = tracing::subscriber::set_default(subscriber);
    metrics::with_local_recorder(&recorder, || {
        let target_id = seed(&resolver, &target, &definition);
        definition.record_id = RecordId::new(uuid::Uuid::new_v4().to_string()).unwrap();
        let unrelated =
            super::super::definition::channel_pointer(base, &definition.record_id).unwrap();
        for _ in 0..49_999 {
            seed(&resolver, &unrelated, &definition);
        }
        assert_eq!(resolver.cache_len(), 50_000);
        assert!(resolver.inner.flights.is_empty());
        let start = Instant::now();
        resolver.invalidate_record(&target.record_id, Some(&target_id));
        let elapsed = start.elapsed();
        assert_eq!(resolver.cache_len(), 49_999);
        assert!(matches!(
            resolver.inner.cache.lookup(&target_id),
            Lookup::Miss
        ));
        println!(
            "invalidate_record: cache_entries=50000 matching_entries=1 restore_flights=0 elapsed_ms={:.3}",
            elapsed.as_secs_f64() * 1000.0
        );
        assert!(
            elapsed <= Duration::from_millis(10),
            "scan exceeded 10 ms: {elapsed:?}"
        );
    });
}
