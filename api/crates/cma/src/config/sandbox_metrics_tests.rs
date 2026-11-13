use super::*;

fn config(value: Option<&str>, enabled: bool) -> Result<MetricsConfig, String> {
    MetricsConfig::from_lookup(&mut |key| {
        Ok(match key {
            "CHRONO_USAGE_REPORTING_MONGO_MAX_POOL_SIZE" => value.map(str::to_owned),
            "CHRONO_USAGE_METRICS_ENABLED" => Some(enabled.to_string()),
            _ => None,
        })
    })
}

#[test]
fn reporting_pool_defaults_to_four_with_collection_on_or_off() {
    for enabled in [false, true] {
        let config = config(None, enabled).unwrap();
        assert_eq!(config.reporting_pool_size, 4);
        assert_eq!(config.budgets, MetricsBudgets::default());
    }
}

#[test]
fn reporting_pool_accepts_the_entire_bounded_range() {
    assert_eq!(config(Some("08"), false).unwrap().reporting_pool_size, 8);
    for size in 1..=16 {
        for enabled in [false, true] {
            let config = config(Some(&size.to_string()), enabled).unwrap();
            assert_eq!(config.reporting_pool_size, size);
            assert_eq!(config.budgets, MetricsBudgets::default());
        }
    }
}

#[test]
fn reporting_pool_rejects_unbounded_malformed_and_overflow_values() {
    for value in [
        "",
        "0",
        "17",
        "4294967295",
        "4294967296",
        "999999999999999999999999",
        "-1",
        "+8",
        " 8",
        "8 ",
        "8\n",
        "1.5",
        "eight",
        "８",
    ] {
        for enabled in [false, true] {
            let error = config(Some(value), enabled).unwrap_err();
            assert_eq!(
                error,
                "CHRONO_USAGE_REPORTING_MONGO_MAX_POOL_SIZE must be an integer in 1..=16"
            );
        }
    }
}

#[test]
fn reporting_pool_propagates_lookup_failure_instead_of_defaulting() {
    let error = MetricsConfig::from_lookup(&mut |key| {
        if key == "CHRONO_USAGE_REPORTING_MONGO_MAX_POOL_SIZE" {
            Err("reporting pool environment is not Unicode".into())
        } else {
            Ok(None)
        }
    })
    .unwrap_err();
    assert_eq!(error, "reporting pool environment is not Unicode");
}
