use super::connection_options;

#[tokio::test]
async fn explicit_pool_allocation_survives_observer_configuration() {
    let options = connection_options("mongodb://localhost/?maxPoolSize=30&minPoolSize=5")
        .await
        .unwrap();
    assert_eq!(options.max_pool_size, Some(30));
    assert_eq!(options.min_pool_size, Some(5));
}

#[tokio::test]
async fn unspecified_pool_allocation_retains_driver_defaults() {
    let options = connection_options("mongodb://localhost/").await.unwrap();
    assert_eq!(options.max_pool_size, None);
    assert_eq!(options.min_pool_size, None);
}

#[tokio::test]
async fn invalid_pool_allocation_is_refused() {
    assert!(
        connection_options("mongodb://localhost/?maxPoolSize=invalid")
            .await
            .is_err()
    );
}
