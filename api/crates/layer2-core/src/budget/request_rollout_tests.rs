//! Strict budget reader retained from the foundation, before request policies.
use super::*;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FoundationSettings {
    #[serde(default)]
    enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    platform: Option<PlatformBudgetPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    default_policy: Option<BudgetPolicy>,
    owners: Vec<OwnerBudgetPolicy>,
}

#[test]
fn empty_request_policy_stays_readable_by_the_foundation_reader() {
    let settings = BudgetSettings::default();
    let stored = serde_json::to_value(&settings).unwrap();
    assert!(stored.get("request_models").is_none());
    let previous: FoundationSettings = serde_json::from_value(stored.clone()).unwrap();
    assert_eq!(serde_json::to_value(previous).unwrap(), stored);
    assert_eq!(
        serde_json::from_value::<BudgetSettings>(stored).unwrap(),
        settings
    );
}

#[test]
fn disabled_request_policy_already_requires_a_new_compatible_reader() {
    let settings = BudgetSettings {
        request_models: vec![RequestModelPolicy {
            model: "approved".into(),
            input_overhead_tokens: u64::from(u8::MAX),
        }],
        ..Default::default()
    };
    settings.validate().unwrap();
    assert!(!settings.enabled);
    assert!(!settings.is_empty_disabled());
    let stored = serde_json::to_value(&settings).unwrap();
    assert!(serde_json::from_value::<FoundationSettings>(stored.clone()).is_err());
    assert_eq!(
        serde_json::from_value::<BudgetSettings>(stored).unwrap(),
        settings
    );
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FoundationReservation {
    policy: BudgetPolicy,
    reserved_units: u64,
    observed: u64,
    last_usage_at: u64,
    final_usage_known: bool,
    stop: Option<BudgetError>,
    settled: bool,
}

#[test]
fn absent_final_request_count_preserves_the_foundation_reservation_shape() {
    let mut reservation = TurnReservation::new(
        BudgetPolicy {
            owner_day_limit: u64::from(u32::MAX),
            turn_limit: u64::from(u16::MAX),
            max_calls_per_turn: u64::from(u8::MAX),
            day_timezone: "UTC".into(),
            input_token_units: 1,
            output_token_units: 1,
            max_unobserved_units: 1,
            usage_timeout_seconds: 30,
        },
        0,
    );
    let stored = serde_json::to_value(&reservation).unwrap();
    assert!(stored.get("final_model_calls").is_none());
    let previous: FoundationReservation = serde_json::from_value(stored.clone()).unwrap();
    assert_eq!(serde_json::to_value(previous).unwrap(), stored);
    assert_eq!(
        serde_json::from_value::<TurnReservation>(stored).unwrap(),
        reservation
    );
    reservation.final_model_calls = Some(0);
    let stored = serde_json::to_value(reservation).unwrap();
    assert!(serde_json::from_value::<FoundationReservation>(stored.clone()).is_err());
    assert_eq!(
        serde_json::from_value::<TurnReservation>(stored)
            .unwrap()
            .final_model_calls,
        Some(0)
    );
}
