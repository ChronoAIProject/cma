use cma_sdk::{Client, Error, usage_metrics::OwnerAllocationQuery};

#[tokio::test]
async fn owner_allocations_reject_non_admin_and_invalid_limits_before_http() {
    let client = Client::builder("http://127.0.0.1:1")
        .unwrap()
        .build()
        .unwrap();
    let query = OwnerAllocationQuery {
        metric: "cpu".into(),
        limit: None,
        cursor: None,
    };
    assert!(matches!(
        client
            .usage_metrics()
            .owner_allocations("report", &query)
            .await,
        Err(Error::Validation { .. })
    ));
    for limit in [0, 201] {
        let query = OwnerAllocationQuery {
            limit: Some(limit),
            ..query.clone()
        };
        assert!(matches!(
            client
                .admin_usage_metrics()
                .owner_allocations("report", &query)
                .await,
            Err(Error::Validation { .. })
        ));
    }
}
