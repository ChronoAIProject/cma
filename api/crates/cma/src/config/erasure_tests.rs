use super::configured;
use crate::store::account_erasure::Options;

#[test]
fn deletion_activation_requires_an_implemented_worker_and_independent_verifier() {
    let flags = [
        "CHRONO_ACCOUNT_DELETION_ENABLED",
        "CHRONO_ACCOUNT_DELETION_WORKER_ENABLED",
        "CHRONO_ACCOUNT_DELETION_VERIFY_ENABLED",
    ];
    assert!(!configured(&[]).unwrap().account_deletion_enabled);
    for enabled in 0..8 {
        let values = flags
            .iter()
            .enumerate()
            .map(|(index, name)| {
                (
                    *name,
                    if enabled & (1 << index) == 0 {
                        "false"
                    } else {
                        "true"
                    },
                )
            })
            .collect::<Vec<_>>();
        let result = configured(&values);
        if enabled == 0 {
            assert!(!result.unwrap().account_deletion_enabled);
        } else {
            assert!(result
                .unwrap_err()
                .contains("account deletion execution is unavailable"));
        }
    }
}

#[test]
fn deletion_execution_switches_reject_noncanonical_booleans() {
    for name in [
        "CHRONO_ACCOUNT_DELETION_WORKER_ENABLED",
        "CHRONO_ACCOUNT_DELETION_VERIFY_ENABLED",
    ] {
        for value in ["", "TRUE", "1", "yes", "false ", " true"] {
            assert_eq!(
                configured(&[(name, value)]).unwrap_err(),
                format!("{name} must be true or false")
            );
        }
    }
}

#[test]
fn legal_consent_retention_is_an_independent_opt_in() {
    const KEY: &str = "CHRONO_ERASURE_RETAIN_LEGAL_CONSENTS";
    assert!(!Options::from_config(&configured(&[]).unwrap()).retain_legal_consents);
    let config = configured(&[(KEY, "true")]).unwrap();
    assert!(Options::from_config(&config).retain_legal_consents);
    assert!(config.legal_terms.is_none());
    assert!(configured(&[(KEY, "yes")]).is_err());
}

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

#[test]
fn account_erasure_transaction_delay_never_supplies_writer_qualification() {
    let config = configured(&[]).unwrap();
    assert_eq!(config.account_erasure_transaction_drain_seconds, 120);
    let options = crate::store::account_erasure::Options::from_config(&config);
    assert!(!options.billing_enabled);
    assert_eq!(options.transaction_drain_minimum_secs, 120);
    assert!(options.drain_qualification.is_none());
    assert_eq!(
        configured(&[("CHRONO_ACCOUNT_ERASURE_TRANSACTION_DRAIN_SECONDS", "240")])
            .unwrap()
            .account_erasure_transaction_drain_seconds,
        240
    );
    for invalid in ["0", "60", "119", "86401", "-1", "abc", " 120", "0120"] {
        assert!(
            configured(&[("CHRONO_ACCOUNT_ERASURE_TRANSACTION_DRAIN_SECONDS", invalid)]).is_err(),
            "{invalid}"
        );
    }
}
