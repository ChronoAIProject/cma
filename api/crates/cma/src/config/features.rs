//! Independent rollout policy for the current CMA interfaces.
#[cfg(test)]
#[path = "ingress_coordinator_tests.rs"]
mod ingress_coordinator_tests;
#[cfg(test)]
mod segmentation_tests;
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct FeatureFlags {
    pub command_reconcile_pacing: bool,
    pub coordinated_ingress: bool,
    pub retained_json: bool,
    pub bounded_raw_output: bool,
    pub output_json_budget: bool,
    pub bounded_update_relay: bool,
    pub bounded_live_fragments: bool,
    pub retain_read_frames: bool,
    pub bounded_permission_state: bool,
    pub parent_not_ready_pacing_enabled: bool,
    pub retained_reopen_enabled: bool,
    pub agent_skill_upgrade_enabled: bool,
    pub shared_runtime_output: bool,
    pub model_budget_writes_enabled: bool,
    pub model_request_budget_writes_enabled: bool,
    pub platform_spend_writes_enabled: bool,
    pub platform_counter_shards: bool,
    pub admission_control_writes_enabled: bool,
    pub bounded_title_reads: bool,
    pub access_snapshot_session: bool,
    pub concurrent_sandbox_detail_enrichment: bool,
    pub sandbox_snapshot_reads: bool,
    pub sandbox_batched_reads: bool,
    pub sandbox_detail_timings: bool,
    pub bounded_response_reads: bool,
    pub bounded_transcript_reads: bool,
    pub bounded_usage_reads: bool,
    pub capacity_recovery_enabled: bool,
    pub segment_adoption_enabled: bool,
    pub paged_response_events: bool,
    pub v2_ui_transport: bool,
    pub trigger_command_admission: bool,
    pub nyxid_resource_request: bool,
    pub nyxid_proxy: bool,
    pub silent_sign_in: bool,
    pub document_revocation_reserve: bool,
    pub document_quotas: bool,
    pub document_object_deletion: bool,
    pub document_reclaim: bool,
    pub bounded_approval_reads: bool,
    pub bounded_close_retries: bool,
    pub retained_context_lookup: bool,
    pub event_driven_reconciliation: bool,
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
        let flags = Self {
            command_reconcile_pacing: flag(lookup, "CHRONO_COMMAND_RECONCILE_PACING_ENABLED")?,
            coordinated_ingress: flag(lookup, "CHRONO_INGRESS_COORDINATOR_ENABLED")?,
            retained_json: flag(lookup, "CHRONO_RUNTIME_JSON_RETENTION_BOUNDS_ENABLED")?,
            bounded_raw_output: flag(lookup, "CHRONO_RUNTIME_RAW_OUTPUT_BOUNDS_ENABLED")?,
            output_json_budget: flag(lookup, "CHRONO_RUNTIME_OUTPUT_JSON_BUDGET_ENABLED")?,
            bounded_update_relay: flag(lookup, "CHRONO_RUNTIME_UPDATE_RELAY_BOUNDED_ENABLED")?,
            bounded_live_fragments: flag(lookup, "CHRONO_AGENT_LIVE_BUFFER_BUDGET_ENABLED")?,
            retain_read_frames: flag(lookup, "CHRONO_READ_FRAME_BUDGET_ENABLED")?,
            bounded_permission_state: flag(lookup, "CHRONO_RUNTIME_PERMISSION_BOUNDS_ENABLED")?,
            parent_not_ready_pacing_enabled: flag(
                lookup,
                "CHRONO_AGENT_PARENT_NOT_READY_PACING_ENABLED",
            )?,
            retained_reopen_enabled: flag(lookup, "CHRONO_RETAINED_REOPEN_ENABLED")?,
            agent_skill_upgrade_enabled: flag(lookup, "CHRONO_AGENT_SKILL_UPGRADE_ENABLED")?,
            access_snapshot_session: super::access_snapshot::enabled(lookup)?,
            concurrent_sandbox_detail_enrichment: flag(
                lookup,
                "CHRONO_SANDBOX_DETAIL_ENRICHMENT_CONCURRENT_ENABLED",
            )?,
            sandbox_snapshot_reads: flag(lookup, "CHRONO_SANDBOX_SNAPSHOT_READS_ENABLED")?,
            sandbox_batched_reads: flag(lookup, "CHRONO_SANDBOX_BATCHED_READS_ENABLED")?,
            sandbox_detail_timings: match lookup("CHRONO_SANDBOX_DETAIL_TIMINGS_ENABLED")? {
                None => true,
                Some(value) => flag(
                    &mut |_| Ok(Some(value.clone())),
                    "CHRONO_SANDBOX_DETAIL_TIMINGS_ENABLED",
                )?,
            },
            document_quotas: flag(lookup, "CHRONO_DOCUMENT_QUOTAS_ENABLED")?,
            document_object_deletion: flag(lookup, "CHRONO_DOCUMENT_OBJECT_DELETION_ENABLED")?,
            document_reclaim: flag(lookup, "CHRONO_DOCUMENT_RECLAIM_ENABLED")?,
            shared_runtime_output: flag(lookup, "CHRONO_RUNTIME_SHARED_OUTPUT_ENABLED")?,
            bounded_close_retries: flag(lookup, "CHRONO_AGENT_CLOSE_RETRY_BOUNDED_ENABLED")?,
            retained_context_lookup: flag(lookup, "CHRONO_CMAEG_RETAINED_CONTEXT_LOOKUP_ENABLED")?,
            event_driven_reconciliation: flag(
                lookup,
                "CHRONO_AGENT_RECONCILIATION_EVENT_DRIVEN_ENABLED",
            )?,
            bounded_approval_reads: flag(lookup, "CHRONO_AGENT_APPROVAL_BOUNDED_READS_ENABLED")?,
            model_budget_writes_enabled: flag(lookup, "CHRONO_MODEL_BUDGET_WRITES_ENABLED")?,
            model_request_budget_writes_enabled: flag(
                lookup,
                "CHRONO_MODEL_REQUEST_BUDGET_WRITES_ENABLED",
            )?,
            platform_spend_writes_enabled: flag(lookup, "CHRONO_PLATFORM_SPEND_WRITES_ENABLED")?,
            platform_counter_shards: flag(lookup, "CHRONO_PLATFORM_COUNTER_SHARDS_ENABLED")?,
            admission_control_writes_enabled: flag(
                lookup,
                "CHRONO_ADMISSION_CONTROL_WRITES_ENABLED",
            )?,
            bounded_title_reads: flag(lookup, "CHRONO_AGENT_TITLE_BOUNDED_READS_ENABLED")?,
            bounded_response_reads: flag(lookup, "CHRONO_AGENT_RESPONSE_BOUNDED_READS_ENABLED")?,
            bounded_transcript_reads: flag(
                lookup,
                "CHRONO_AGENT_TRANSCRIPT_BOUNDED_READS_ENABLED",
            )?,
            bounded_usage_reads: flag(lookup, "CHRONO_AGENT_USAGE_BOUNDED_READS_ENABLED")?,
            capacity_recovery_enabled: flag(lookup, "CHRONO_AGENT_CAPACITY_RECOVERY_ENABLED")?,
            segment_adoption_enabled: flag(lookup, "CHRONO_AGENT_LOG_SEGMENT_ADOPTION_ENABLED")?,
            paged_response_events: flag(lookup, "CHRONO_RESPONSE_PAGED_EVENTS_ENABLED")?,
            v2_ui_transport: flag(lookup, "CHRONO_V2_UI_TRANSPORT_ENABLED")?,
            trigger_command_admission: flag(lookup, "CHRONO_TRIGGER_COMMAND_ADMISSION_ENABLED")?,
            nyxid_resource_request: flag(lookup, "CHRONO_NYXID_RESOURCE_REQUEST_ENABLED")?,
            nyxid_proxy: flag(lookup, "CHRONO_NYXID_PROXY_ENABLED")?,
            silent_sign_in: flag(lookup, "CHRONO_SILENT_SIGN_IN_ENABLED")?,
            document_revocation_reserve: flag(
                lookup,
                "CHRONO_DOCUMENT_REVOCATION_RESERVE_ENABLED",
            )?,
        };
        if flags.sandbox_batched_reads && !flags.sandbox_snapshot_reads {
            return Err("CHRONO_SANDBOX_BATCHED_READS_ENABLED requires CHRONO_SANDBOX_SNAPSHOT_READS_ENABLED".into());
        }
        if flags.coordinated_ingress
            && !(flags.retained_json
                && flags.bounded_raw_output
                && flags.bounded_permission_state
                && flags.bounded_live_fragments)
        {
            return Err("CHRONO_INGRESS_COORDINATOR_ENABLED requires JSON retention, raw output, permission and live fragment bounds".into());
        }
        Ok(flags)
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
#[path = "sandbox_detail_diagnostics_tests.rs"]
mod sandbox_read_tests;

#[cfg(test)]
mod reconciliation_tests;

#[cfg(test)]
mod sandbox_batch_tests {
    use super::*;
    #[test]
    fn batching_is_default_off_strict_and_requires_snapshot_sessions() {
        for snapshot in [false, true] {
            for value in [
                None,
                Some("false"),
                Some("true"),
                Some("TRUE"),
                Some("1"),
                Some(" true "),
                Some(""),
            ] {
                let flags = FeatureFlags::from_lookup(&mut |key| {
                    Ok(match key {
                        "CHRONO_SANDBOX_SNAPSHOT_READS_ENABLED" => Some(snapshot.to_string()),
                        "CHRONO_SANDBOX_BATCHED_READS_ENABLED" => value.map(str::to_owned),
                        _ => None,
                    })
                });
                match value {
                    None | Some("false") => assert!(!flags.unwrap().sandbox_batched_reads),
                    Some("true") if snapshot => assert!(flags.unwrap().sandbox_batched_reads),
                    _ => assert!(flags.is_err()),
                }
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
mod paged_event_tests {
    use super::*;
    #[test]
    fn paged_events_are_default_off_and_require_their_own_strict_boolean() {
        for (value, enabled) in [(None, false), (Some("false"), false), (Some("true"), true)] {
            let flags = FeatureFlags::from_lookup(&mut |key| {
                Ok((key == "CHRONO_RESPONSE_PAGED_EVENTS_ENABLED")
                    .then(|| value.map(str::to_owned))
                    .flatten())
            })
            .unwrap();
            assert_eq!(flags.paged_response_events, enabled);
            assert!(!flags.bounded_response_reads);
        }
        for value in ["", "1", "TRUE", " true "] {
            assert!(FeatureFlags::from_lookup(&mut |key| Ok((key
                == "CHRONO_RESPONSE_PAGED_EVENTS_ENABLED")
                .then(|| value.into())))
            .is_err());
        }
    }
}

#[cfg(test)]
mod transcript_tests {
    use super::*;
    #[test]
    fn transcript_reads_require_an_independent_strict_opt_in() {
        for (value, enabled) in [(None, false), (Some("false"), false), (Some("true"), true)] {
            let flags = FeatureFlags::from_lookup(&mut |key| {
                Ok((key == "CHRONO_AGENT_TRANSCRIPT_BOUNDED_READS_ENABLED")
                    .then(|| value.map(str::to_owned))
                    .flatten())
            })
            .unwrap();
            assert_eq!(flags.bounded_transcript_reads, enabled);
            assert!(!flags.bounded_response_reads && !flags.bounded_title_reads);
        }
        for value in ["", "1", "TRUE", " true "] {
            assert!(FeatureFlags::from_lookup(&mut |key| Ok((key
                == "CHRONO_AGENT_TRANSCRIPT_BOUNDED_READS_ENABLED")
                .then(|| value.into())))
            .is_err());
        }
    }
}

#[cfg(test)]
mod permission_bound_tests;

#[cfg(test)]
mod read_frame_tests {
    use super::*;

    #[test]
    fn read_frame_ownership_requires_an_independent_strict_boolean() {
        const KEY: &str = "CHRONO_READ_FRAME_BUDGET_ENABLED";
        for (value, enabled) in [(None, false), (Some("false"), false), (Some("true"), true)] {
            let flags = FeatureFlags::from_lookup(&mut |key| {
                Ok((key == KEY).then(|| value.map(str::to_owned)).flatten())
            })
            .unwrap();
            assert_eq!(flags.retain_read_frames, enabled);
            assert!(!flags.bounded_response_reads && !flags.bounded_title_reads);
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
mod live_buffer_tests {
    use super::*;

    #[test]
    fn live_buffer_budget_requires_its_own_strict_boolean() {
        const KEY: &str = "CHRONO_AGENT_LIVE_BUFFER_BUDGET_ENABLED";
        for (value, enabled) in [(None, false), (Some("false"), false), (Some("true"), true)] {
            let flags = FeatureFlags::from_lookup(&mut |key| {
                Ok((key == KEY).then(|| value.map(str::to_owned)).flatten())
            })
            .unwrap();
            assert_eq!(flags.bounded_live_fragments, enabled);
            assert!(!flags.bounded_close_retries);
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
mod relay_tests {
    use super::*;

    #[test]
    fn bounded_update_relay_requires_a_separate_strict_boolean() {
        const KEY: &str = "CHRONO_RUNTIME_UPDATE_RELAY_BOUNDED_ENABLED";
        for (value, enabled) in [(None, false), (Some("false"), false), (Some("true"), true)] {
            let flags = FeatureFlags::from_lookup(&mut |key| {
                Ok((key == KEY).then(|| value.map(str::to_owned)).flatten())
            })
            .unwrap();
            assert_eq!(flags.bounded_update_relay, enabled);
            assert!(!flags.bounded_close_retries);
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
mod rollout_tail_tests;

#[cfg(test)]
mod raw_output_tests;

#[cfg(test)]
mod retained_json_tests;

#[cfg(test)]
#[path = "sandbox_enrichment_tests.rs"]
mod sandbox_enrichment_tests;

#[cfg(test)]
mod document_maintenance_tests;

#[cfg(test)]
#[path = "admission_control_tests.rs"]
mod admission_control_tests;
