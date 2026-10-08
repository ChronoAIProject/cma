//! Keep publish defaults deployable while the feature is gated off.
use super::{configmap_value, CONFIGMAP};

#[test]
fn agent_event_budgets_match_code_and_ship_disabled() {
    let defaults = cmaeg::AgentEventsConfig::default();
    assert!(!defaults.enabled);
    assert_eq!(
        configmap_value(CONFIGMAP, "CMAEG_AGENT_EVENTS_ENABLED"),
        Some("false")
    );
    for (key, expected) in [
        (
            "CMAEG_AGENT_EVENTS_RATE_PER_MINUTE",
            u128::from(defaults.rate_per_minute),
        ),
        (
            "CMAEG_AGENT_EVENTS_OWNER_RATE_PER_MINUTE",
            u128::from(defaults.owner_rate_per_minute),
        ),
        (
            "CMAEG_AGENT_EVENTS_ACCEPT_MS",
            defaults.accept_budget.as_millis(),
        ),
    ] {
        assert_eq!(
            configmap_value(CONFIGMAP, key)
                .unwrap()
                .parse::<u128>()
                .unwrap(),
            expected
        );
    }
}
