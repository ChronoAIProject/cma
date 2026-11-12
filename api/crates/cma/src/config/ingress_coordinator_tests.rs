use super::*;

const COORDINATOR: &str = "CHRONO_INGRESS_COORDINATOR_ENABLED";
const REQUIRED: [&str; 4] = [
    "CHRONO_RUNTIME_JSON_RETENTION_BOUNDS_ENABLED",
    "CHRONO_RUNTIME_RAW_OUTPUT_BOUNDS_ENABLED",
    "CHRONO_RUNTIME_PERMISSION_BOUNDS_ENABLED",
    "CHRONO_AGENT_LIVE_BUFFER_BUDGET_ENABLED",
];

#[test]
fn coordinator_is_strict_default_off_and_requires_every_covered_owner() {
    assert!(
        !FeatureFlags::from_lookup(&mut |_| Ok(None))
            .unwrap()
            .coordinated_ingress
    );
    for value in [
        None,
        Some("false"),
        Some("true"),
        Some("TRUE"),
        Some("1"),
        Some(""),
        Some(" true "),
    ] {
        for mask in 0..16 {
            let result = FeatureFlags::from_lookup(&mut |key| {
                Ok(if key == COORDINATOR {
                    value.map(str::to_owned)
                } else {
                    REQUIRED
                        .iter()
                        .position(|required| *required == key)
                        .map(|index| (mask & (1 << index) != 0).to_string())
                })
            });
            match value {
                None | Some("false") => assert!(!result.unwrap().coordinated_ingress),
                Some("true") if mask == 15 => assert!(result.unwrap().coordinated_ingress),
                _ => assert!(result.is_err()),
            }
        }
    }
}
