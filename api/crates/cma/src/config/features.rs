//! Independent rollout policy for the current CMA interfaces.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct FeatureFlags {
    pub parent_not_ready_pacing_enabled: bool,
    pub bounded_permission_state: bool,
    pub retained_reopen_enabled: bool,
    pub model_budget_writes_enabled: bool,
    pub model_request_budget_writes_enabled: bool,
    pub bounded_title_reads: bool,
    pub sandbox_snapshot_reads: bool,
    pub sandbox_detail_timings: bool,
    pub bounded_response_reads: bool,
    pub capacity_recovery_enabled: bool,
    pub v2_ui_transport: bool,
    pub trigger_command_admission: bool,
    pub nyxid_resource_request: bool,
    pub nyxid_proxy: bool,
    pub document_revocation_reserve: bool,
    pub bounded_approval_reads: bool,
    pub bounded_close_retries: bool,
}
impl FeatureFlags {
    pub(crate) fn from_lookup(
        lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
    ) -> Result<Self, String> {
        fn flag(
            lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
            key: &str,
        ) -> Result<bool, String> {
            match lookup(key)?.as_deref() {
                None | Some("false") => Ok(false),
                Some("true") => Ok(true),
                _ => Err(format!("{key} must be true or false")),
            }
        }
        Ok(Self {
            parent_not_ready_pacing_enabled: flag(
                lookup,
                "CHRONO_AGENT_PARENT_NOT_READY_PACING_ENABLED",
            )?,
            bounded_permission_state: flag(lookup, "CHRONO_RUNTIME_PERMISSION_BOUNDS_ENABLED")?,
            retained_reopen_enabled: flag(lookup, "CHRONO_RETAINED_REOPEN_ENABLED")?,
            sandbox_snapshot_reads: flag(lookup, "CHRONO_SANDBOX_SNAPSHOT_READS_ENABLED")?,
            sandbox_detail_timings: flag(lookup, "CHRONO_SANDBOX_DETAIL_TIMINGS_ENABLED")?,
            bounded_close_retries: flag(lookup, "CHRONO_AGENT_CLOSE_RETRY_BOUNDED_ENABLED")?,
            bounded_approval_reads: flag(lookup, "CHRONO_AGENT_APPROVAL_BOUNDED_READS_ENABLED")?,
            model_budget_writes_enabled: flag(lookup, "CHRONO_MODEL_BUDGET_WRITES_ENABLED")?,
            model_request_budget_writes_enabled: flag(
                lookup,
                "CHRONO_MODEL_REQUEST_BUDGET_WRITES_ENABLED",
            )?,
            bounded_title_reads: flag(lookup, "CHRONO_AGENT_TITLE_BOUNDED_READS_ENABLED")?,
            bounded_response_reads: flag(lookup, "CHRONO_AGENT_RESPONSE_BOUNDED_READS_ENABLED")?,
            capacity_recovery_enabled: flag(lookup, "CHRONO_AGENT_CAPACITY_RECOVERY_ENABLED")?,
            v2_ui_transport: flag(lookup, "CHRONO_V2_UI_TRANSPORT_ENABLED")?,
            trigger_command_admission: flag(lookup, "CHRONO_TRIGGER_COMMAND_ADMISSION_ENABLED")?,
            nyxid_resource_request: flag(lookup, "CHRONO_NYXID_RESOURCE_REQUEST_ENABLED")?,
            nyxid_proxy: flag(lookup, "CHRONO_NYXID_PROXY_ENABLED")?,
            document_revocation_reserve: flag(
                lookup,
                "CHRONO_DOCUMENT_REVOCATION_RESERVE_ENABLED",
            )?,
        })
    }
}

#[cfg(test)]
mod capacity_tests;

#[cfg(test)]
mod parent_pacing_tests {
    use super::*;

    #[test]
    fn parent_readiness_pacing_is_independent_and_default_off() {
        for (value, expected) in [(None, false), (Some("false"), false), (Some("true"), true)] {
            let flags = FeatureFlags::from_lookup(&mut |key| {
                Ok((key == "CHRONO_AGENT_PARENT_NOT_READY_PACING_ENABLED")
                    .then(|| value.map(str::to_owned))
                    .flatten())
            })
            .unwrap();
            assert_eq!(flags.parent_not_ready_pacing_enabled, expected);
            assert!(!flags.bounded_close_retries);
            assert!(!flags.bounded_response_reads);
        }
        for value in ["", "1", "TRUE", " true "] {
            assert!(FeatureFlags::from_lookup(&mut |key| {
                Ok((key == "CHRONO_AGENT_PARENT_NOT_READY_PACING_ENABLED").then(|| value.into()))
            })
            .is_err());
        }
    }
}

#[cfg(test)]
mod approval_tests {
    use super::*;

    #[test]
    fn approval_reader_rollout_requires_an_explicit_boolean() {
        for (value, expected) in [(None, false), (Some("false"), false), (Some("true"), true)] {
            let flags = FeatureFlags::from_lookup(&mut |key| {
                Ok((key == "CHRONO_AGENT_APPROVAL_BOUNDED_READS_ENABLED")
                    .then(|| value.map(str::to_owned))
                    .flatten())
            })
            .unwrap();
            assert_eq!(flags.bounded_approval_reads, expected);
            assert!(!flags.bounded_title_reads);
        }
        for value in ["", "1", "TRUE", " true "] {
            assert!(FeatureFlags::from_lookup(&mut |key| Ok((key
                == "CHRONO_AGENT_APPROVAL_BOUNDED_READS_ENABLED")
                .then(|| value.to_owned())))
            .is_err());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_title_reads_require_an_explicit_boolean_opt_in() {
        for (value, expected) in [(None, false), (Some("false"), false), (Some("true"), true)] {
            let flags = FeatureFlags::from_lookup(&mut |key| {
                Ok(if key == "CHRONO_AGENT_TITLE_BOUNDED_READS_ENABLED" {
                    value.map(str::to_owned)
                } else {
                    None
                })
            })
            .unwrap();
            assert_eq!(flags.bounded_title_reads, expected);
        }
        for value in ["", "1", "TRUE", " true "] {
            assert!(FeatureFlags::from_lookup(&mut |key| {
                Ok((key == "CHRONO_AGENT_TITLE_BOUNDED_READS_ENABLED").then(|| value.into()))
            })
            .is_err());
        }
    }
}

#[cfg(test)]
mod close_retry_tests {
    use super::*;

    #[test]
    fn close_retry_optimization_requires_its_own_strict_boolean() {
        for (value, expected) in [(None, false), (Some("false"), false), (Some("true"), true)] {
            let flags = FeatureFlags::from_lookup(&mut |key| {
                Ok(if key == "CHRONO_AGENT_CLOSE_RETRY_BOUNDED_ENABLED" {
                    value.map(str::to_owned)
                } else {
                    None
                })
            })
            .unwrap();
            assert_eq!(flags.bounded_close_retries, expected);
            assert!(!flags.bounded_title_reads && !flags.bounded_approval_reads);
        }
        for value in ["", "1", "TRUE", " true "] {
            assert!(FeatureFlags::from_lookup(&mut |key| {
                Ok((key == "CHRONO_AGENT_CLOSE_RETRY_BOUNDED_ENABLED").then(|| value.into()))
            })
            .is_err());
        }
    }
}

#[cfg(test)]
mod sandbox_read_tests {
    use super::*;

    #[test]
    fn sandbox_snapshot_and_timings_are_independent_strict_opt_ins() {
        for key in [
            "CHRONO_SANDBOX_SNAPSHOT_READS_ENABLED",
            "CHRONO_SANDBOX_DETAIL_TIMINGS_ENABLED",
        ] {
            for (value, enabled) in [(None, false), (Some("false"), false), (Some("true"), true)] {
                let flags = FeatureFlags::from_lookup(&mut |name| {
                    Ok((name == key).then(|| value.map(str::to_owned)).flatten())
                })
                .unwrap();
                assert_eq!(
                    flags.sandbox_snapshot_reads,
                    enabled && key.contains("SNAPSHOT")
                );
                assert_eq!(
                    flags.sandbox_detail_timings,
                    enabled && key.contains("TIMINGS")
                );
            }
            for invalid in ["", "1", "TRUE", " true "] {
                assert!(FeatureFlags::from_lookup(&mut |name| {
                    Ok((name == key).then(|| invalid.to_owned()))
                })
                .is_err());
            }
        }
    }
}

#[cfg(test)]
mod response_tests {
    use super::*;

    #[test]
    fn bounded_response_reads_require_their_own_strict_opt_in() {
        for (value, expected) in [(None, false), (Some("false"), false), (Some("true"), true)] {
            let flags = FeatureFlags::from_lookup(&mut |key| {
                Ok((key == "CHRONO_AGENT_RESPONSE_BOUNDED_READS_ENABLED")
                    .then(|| value.map(str::to_owned))
                    .flatten())
            })
            .unwrap();
            assert_eq!(flags.bounded_response_reads, expected);
            assert!(!flags.bounded_approval_reads && !flags.bounded_title_reads);
        }
        for value in ["", "1", "TRUE", " true "] {
            assert!(FeatureFlags::from_lookup(&mut |key| Ok((key
                == "CHRONO_AGENT_RESPONSE_BOUNDED_READS_ENABLED")
                .then(|| value.into())))
            .is_err());
        }
    }
}

#[cfg(test)]
mod permission_bound_tests {
    use super::*;

    #[test]
    fn permission_state_requires_its_own_strict_opt_in() {
        for (value, expected) in [(None, false), (Some("false"), false), (Some("true"), true)] {
            let flags = FeatureFlags::from_lookup(&mut |key| {
                Ok((key == "CHRONO_RUNTIME_PERMISSION_BOUNDS_ENABLED")
                    .then(|| value.map(str::to_owned))
                    .flatten())
            })
            .unwrap();
            assert_eq!(flags.bounded_permission_state, expected);
            assert!(!flags.bounded_response_reads);
        }
        for value in ["", "1", "TRUE", " true "] {
            assert!(FeatureFlags::from_lookup(&mut |key| {
                Ok((key == "CHRONO_RUNTIME_PERMISSION_BOUNDS_ENABLED").then(|| value.into()))
            })
            .is_err());
        }
    }
}
