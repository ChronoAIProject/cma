use super::*;
use crate::config::sandbox_metrics::DEFAULT_REPORTING_POOL_SIZE;

#[tokio::test]
async fn reporting_pool_config_overrides_uri_without_changing_pressure_limits() {
    // Parsing a literal localhost URI does not connect to MongoDB.
    for size in [DEFAULT_REPORTING_POOL_SIZE, 1, 8, 16] {
        let mut options = ClientOptions::parse(
            "mongodb://localhost:27017/?maxPoolSize=30&minPoolSize=20&maxConnecting=2&connectTimeoutMS=9000&serverSelectionTimeoutMS=9000",
        )
        .await
        .unwrap();
        configure_pool(&mut options, size);
        let circuit = Arc::default();
        budget::observe(&mut options, circuit);
        assert_eq!(options.max_pool_size, Some(size));
        assert_eq!(options.min_pool_size, Some(0));
        assert_eq!(options.max_connecting, Some(1));
        assert_eq!(options.connect_timeout, Some(Duration::from_secs(2)));
        assert_eq!(
            options.server_selection_timeout,
            Some(Duration::from_secs(2))
        );
        assert!(options.cmap_event_handler.is_some());
    }
}
