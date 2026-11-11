//! Independent transport and quota rollout flag contracts.
use super::*;

#[cfg(test)]
mod output_json_tests {
    use super::*;

    #[test]
    fn output_json_budget_requires_a_separate_strict_boolean() {
        const KEY: &str = "CHRONO_RUNTIME_OUTPUT_JSON_BUDGET_ENABLED";
        for (value, enabled) in [(None, false), (Some("false"), false), (Some("true"), true)] {
            let flags = FeatureFlags::from_lookup(&mut |key| {
                Ok((key == KEY).then(|| value.map(str::to_owned)).flatten())
            })
            .unwrap();
            assert_eq!(flags.output_json_budget, enabled);
            assert!(!flags.bounded_response_reads && !flags.bounded_close_retries);
        }
        for value in ["", "1", "TRUE", " true "] {
            assert!(FeatureFlags::from_lookup(&mut |key| {
                Ok((key == KEY).then(|| value.to_owned()))
            })
            .is_err());
        }
    }
}

#[cfg(test)]
mod shared_output_tests {
    use super::*;

    #[test]
    fn shared_runtime_output_requires_its_own_strict_opt_in() {
        for (value, expected) in [(None, false), (Some("false"), false), (Some("true"), true)] {
            let flags = FeatureFlags::from_lookup(&mut |key| {
                Ok((key == "CHRONO_RUNTIME_SHARED_OUTPUT_ENABLED")
                    .then(|| value.map(str::to_owned))
                    .flatten())
            })
            .unwrap();
            assert_eq!(flags.shared_runtime_output, expected);
            assert!(!flags.bounded_response_reads);
        }
        for value in ["", "1", "TRUE", " true "] {
            assert!(FeatureFlags::from_lookup(&mut |key| {
                Ok((key == "CHRONO_RUNTIME_SHARED_OUTPUT_ENABLED").then(|| value.into()))
            })
            .is_err());
        }
    }
}

#[cfg(test)]
mod quota_tests {
    use super::*;

    #[test]
    fn document_quotas_require_an_independent_strict_opt_in() {
        let key = "CHRONO_DOCUMENT_QUOTAS_ENABLED";
        for (value, enabled) in [(None, false), (Some("false"), false), (Some("true"), true)] {
            let flags = FeatureFlags::from_lookup(&mut |name| {
                Ok((name == key).then(|| value.map(str::to_owned)).flatten())
            })
            .unwrap();
            assert_eq!(flags.document_quotas, enabled);
            assert!(!flags.document_revocation_reserve);
        }
        for value in ["", "1", "TRUE", " true "] {
            assert!(FeatureFlags::from_lookup(&mut |name| Ok(
                (name == key).then(|| value.to_owned())
            ))
            .is_err());
        }
    }
}

#[cfg(test)]
mod raw_output_tests {
    use super::*;

    #[test]
    fn raw_output_bounds_require_their_own_strict_opt_in() {
        let key = "CHRONO_RUNTIME_RAW_OUTPUT_BOUNDS_ENABLED";
        for (value, expected) in [(None, false), (Some("false"), false), (Some("true"), true)] {
            let flags = FeatureFlags::from_lookup(&mut |name| {
                Ok((name == key).then(|| value.map(str::to_owned)).flatten())
            })
            .unwrap();
            assert_eq!(flags.bounded_raw_output, expected);
            assert!(!flags.retained_reopen_enabled && !flags.bounded_approval_reads);
        }
        for value in ["", "1", "TRUE", " true "] {
            assert!(FeatureFlags::from_lookup(&mut |name| {
                Ok((name == key).then(|| value.to_owned()))
            })
            .is_err());
        }
    }
}
