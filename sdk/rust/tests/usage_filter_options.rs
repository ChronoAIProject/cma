use cma_sdk::{Client, Error, usage_metrics::OptionQuery};
use serde_json::json;

#[tokio::test]
async fn options_reject_owner_scope_and_invalid_limits_before_http() {
    let client = Client::builder("http://127.0.0.1:1")
        .unwrap()
        .build()
        .unwrap();
    let query: OptionQuery = serde_json::from_value(json!({"kind":"owner"})).unwrap();
    assert!(matches!(
        client.usage_metrics().options(&query).await,
        Err(Error::Validation { .. })
    ));
    for limit in [0, 51] {
        let invalid = OptionQuery {
            limit: Some(limit),
            ..query.clone()
        };
        assert!(matches!(
            client.admin_usage_metrics().options(&invalid).await,
            Err(Error::Validation { .. })
        ));
    }
}
