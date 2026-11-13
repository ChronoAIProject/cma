//! Independent rollout policy for the current CMA interfaces.
#[cfg(test)]
mod agent_event_page_tests;
#[cfg(test)]
#[path = "ingress_coordinator_tests.rs"]
mod ingress_coordinator_tests;
#[cfg(test)]
mod segmentation_tests;
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct FeatureFlags {
    pub agent_event_stream_wake: bool,
    pub agent_event_stream_fallback: std::time::Duration,
    pub runtime_loss_bounded_proof_enabled: bool,
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
    pub single_fence_event_pages: bool,
    pub concurrent_sandbox_detail_enrichment: bool,
    pub sandbox_agent_projection: bool,
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
    pub bounded_approval_reads: bool,
    pub bounded_close_retries: bool,
    pub retry_single_marker_enabled: bool,
    pub work_retry_policy_enabled: bool,
    pub native_create_retry_enabled: bool,
    pub agent_log_scan_cooperative_enabled: bool,
    pub retained_context_lookup: bool,
    pub event_driven_reconciliation: bool,
    pub sandbox_reconciliation_reserved_lane: bool,
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
            agent_event_stream_wake: flag(lookup, "CHRONO_AGENT_EVENT_STREAM_WAKE_ENABLED")?,
            agent_event_stream_fallback: super::public_streams::event_fallback(lookup)?,
            runtime_loss_bounded_proof_enabled: flag(
                lookup,
                "CHRONO_RUNTIME_LOSS_BOUNDED_PROOF_ENABLED",
            )?,
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
            single_fence_event_pages: flag(lookup, "CHRONO_AGENT_EVENT_PAGE_SINGLE_FENCE_ENABLED")?,
            concurrent_sandbox_detail_enrichment: flag(
                lookup,
                "CHRONO_SANDBOX_DETAIL_ENRICHMENT_CONCURRENT_ENABLED",
            )?,
            sandbox_agent_projection: flag(lookup, "CHRONO_SANDBOX_AGENT_PROJECTION_ENABLED")?,
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
            shared_runtime_output: flag(lookup, "CHRONO_RUNTIME_SHARED_OUTPUT_ENABLED")?,
            bounded_close_retries: flag(lookup, "CHRONO_AGENT_CLOSE_RETRY_BOUNDED_ENABLED")?,
            retry_single_marker_enabled: flag(lookup, "CHRONO_AGENT_RETRY_SINGLE_MARKER_ENABLED")?,
            work_retry_policy_enabled: flag(lookup, "CHRONO_WORK_RETRY_POLICY_ENABLED")?,
            native_create_retry_enabled: flag(
                lookup,
                "CHRONO_SANDBOX_NATIVE_CREATE_RETRY_ENABLED",
            )?,
            agent_log_scan_cooperative_enabled: flag(
                lookup,
                "CHRONO_AGENT_LOG_SCAN_COOPERATIVE_ENABLED",
            )?,
            retained_context_lookup: flag(lookup, "CHRONO_CMAEG_RETAINED_CONTEXT_LOOKUP_ENABLED")?,
            event_driven_reconciliation: flag(
                lookup,
                "CHRONO_AGENT_RECONCILIATION_EVENT_DRIVEN_ENABLED",
            )?,
            sandbox_reconciliation_reserved_lane: flag(
                lookup,
                "CHRONO_SANDBOX_RECONCILIATION_RESERVED_LANE_ENABLED",
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
mod parent_pacing_tests;

#[cfg(test)]
mod approval_tests;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod close_retry_tests;

#[cfg(test)]
#[path = "sandbox_detail_diagnostics_tests.rs"]
mod sandbox_read_tests;

#[cfg(test)]
mod reconciliation_tests;

#[cfg(test)]
mod sandbox_batch_tests;

#[cfg(test)]
mod response_tests;

#[cfg(test)]
mod paged_event_tests;

#[cfg(test)]
mod transcript_tests;

#[cfg(test)]
mod permission_bound_tests;

#[cfg(test)]
mod read_frame_tests;

#[cfg(test)]
mod live_buffer_tests;

#[cfg(test)]
mod relay_tests;

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
mod retry_single_marker_tests;

#[cfg(test)]
mod work_retry_tests;

#[cfg(test)]
#[path = "scan_cooperation_tests.rs"]
mod scan_cooperation_tests;

#[cfg(test)]
mod document_maintenance_tests;

#[cfg(test)]
#[path = "admission_control_tests.rs"]
mod admission_control_tests;

#[cfg(test)]
mod runtime_loss_tests;

#[cfg(test)]
#[path = "sandbox_projection_tests.rs"]
mod sandbox_projection_tests;

#[cfg(test)]
#[path = "features/native_create_retry_tests.rs"]
mod native_create_retry_tests;
