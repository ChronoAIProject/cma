use super::connection_options;

#[tokio::test]
async fn explicit_pool_allocation_survives_observer_configuration() {
    let options = connection_options("mongodb://localhost/?maxPoolSize=30&minPoolSize=5", "cma")
        .await
        .unwrap();
    assert_eq!(options.max_pool_size, Some(30));
    assert_eq!(options.min_pool_size, Some(5));
}

#[tokio::test]
async fn unspecified_pool_allocation_retains_driver_defaults() {
    let options = connection_options("mongodb://localhost/", "cma")
        .await
        .unwrap();
    assert_eq!(options.max_pool_size, None);
    assert_eq!(options.min_pool_size, None);
}

#[tokio::test]
async fn invalid_pool_allocation_is_refused() {
    assert!(
        connection_options("mongodb://localhost/?maxPoolSize=invalid", "cma")
            .await
            .is_err()
    );
}

#[tokio::test]
async fn configured_database_carries_the_transaction_barrier_and_keeps_auth_source() {
    // The URI's own database stays the authentication source; only the
    // client's default database (read by the transaction barrier) changes.
    let options = connection_options("mongodb://user:secret@localhost/admin_db", "cma")
        .await
        .unwrap();
    assert_eq!(options.default_database.as_deref(), Some("cma"));
    assert_eq!(
        options
            .credential
            .as_ref()
            .and_then(|credential| credential.source.as_deref()),
        Some("admin_db")
    );
}
