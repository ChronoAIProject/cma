use super::configured;
use crate::store::account_erasure::Options;

#[test]
fn record_erasure_uses_the_configured_delivery_and_optional_store_modes() {
    let mut config = configured(&[]).unwrap();
    let options = Options::from_config(&config);
    assert!(!config.account_deletion_enabled);
    assert!(options.drain_qualification.is_none());
    assert!(!options.billing_enabled);
    assert_eq!(options.allocation_rollups_enabled, Some(false));
    assert_eq!(options.agent_shared_observation, Some(false));
    config.usage_billing = Some(crate::config::UsageBillingConfig {
        api_url: "https://billing.example/api/v1/".parse().unwrap(),
        api_key: "synthetic-erasure-test".to_owned().into(),
        plan_code: "test".into(),
    });
    config.features.agent_shared_observation = true;
    config.usage_metrics.views.allocation_rollups_enabled = true;
    let options = Options::from_config(&config);
    assert!(options.billing_enabled);
    assert_eq!(options.allocation_rollups_enabled, Some(true));
    assert_eq!(options.agent_shared_observation, Some(true));
    assert!(!config.account_deletion_enabled);
    assert!(options.drain_qualification.is_none());
}
