//! Independent rollout policy for the current CMA interfaces.
#[cfg(test)]
mod agent_event_page_tests;
#[cfg(test)]
mod bounded_inactivity_log_tests;
#[cfg(test)]
mod failure_containment_tests;
mod gates;
pub(super) mod home_backup;
#[cfg(test)]
mod home_backup_expiry_tests;
#[cfg(test)]
mod home_backup_tests;
#[cfg(test)]
#[path = "ingress_coordinator_tests.rs"]
mod ingress_coordinator_tests;
mod machine_updates;
pub(crate) use machine_updates::ManagedMachineUpdates;
#[cfg(test)]
mod native_close_lane_tests;
#[cfg(test)]
mod resource_list_tests;
mod rollout_tail;
pub(super) use gates::flag;
#[cfg(test)]
pub(super) use gates::GRADUATED;
#[cfg(test)]
mod segmentation_tests;
#[cfg(test)]
mod stale_turn_tests;
#[cfg(test)]
mod strict_inactivity_tests;
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct FeatureFlags {
    pub image_manager_retirement: bool,
    pub session_guidance_v4: bool,
    pub session_guidance_v5: bool,
    pub session_guidance_v6: bool,
    pub am_startup_health_serving: bool,
    pub am_empty_warm_pool_startup: bool,
    pub visitor_launch_admission: bool,
    pub usage_signed_cursors_enabled: bool,
    pub usage_snapshot_cursors_enabled: bool,
    pub usage_strict_cursors_enabled: bool,
    pub visitor_two_sandbox_defaults: bool,
    pub admin_runtime_inventory: bool,
    pub binding_diagnostics: bool,
    pub agent_inspection_parking: bool,
    pub ownership_retry_hints_enabled: bool,
    pub home_backup_enabled: bool,
    pub home_backup_expiry_enabled: bool,
    pub bot_effect_reservations: bool,
    pub agent_model_discovery_cache: bool,
    pub agent_environment_async: bool,
    pub agent_settings_owner_relay: bool,
    pub agent_file_write_handoff: bool,
    pub agent_event_stream_wake: bool,
    pub agui_bounded_replay: bool,
    pub agent_shared_observation: bool,
    pub agent_event_stream_fallback: std::time::Duration,
    pub runtime_loss_bounded_proof_enabled: bool,
    pub native_retirement_finality_enabled: bool,
    pub native_retirement_capture_enabled: bool,
    pub reviewed_sandbox_retirement_enabled: bool,
    pub command_reconcile_pacing: bool,
    pub command_cancel_wake: bool,
    pub ownership_cleanup_pacing: bool,
    pub batch_ownership_renewal: bool,
    pub am_failure_containment_enabled: bool,
    /// Contain one receiver's transient housekeeping failure instead of
    /// stopping the process (#3069). Default off until staging verifies it.
    pub receiver_housekeeping_containment: bool,
    pub coordinated_ingress: bool,
    pub retained_json: bool,
    pub bounded_raw_output: bool,
    pub output_json_budget: bool,
    pub bounded_update_relay: bool,
    pub native_close_agent_lane: bool,
    pub bounded_live_fragments: bool,
    pub quiet_output: Option<layer2_core::sandbox::AgentOutputCoalescePolicy>,
    pub retain_read_frames: bool,
    pub bounded_permission_state: bool,
    pub parent_not_ready_pacing_enabled: bool,
    pub startup_sibling_hold_enabled: bool,
    pub retained_reopen_enabled: bool,
    pub runtime_image_origin_enabled: bool,
    pub managed_machine_updates: ManagedMachineUpdates,
    pub retained_suspended_reopen_enabled: bool,
    pub retained_repeat_reopen_enabled: bool,
    pub agent_skill_upgrade_enabled: bool,
    pub shared_runtime_output: bool,
    pub model_budget_writes_enabled: bool,
    pub model_credit_required: bool,
    pub model_request_budget_writes_enabled: bool,
    pub platform_spend_writes_enabled: bool,
    pub platform_counter_shards: bool,
    pub admission_control_writes_enabled: bool,
    pub admission_ceilings_writes_enabled: bool,
    pub pending_home_reconciliation: bool,
    pub agent_control_write_coordination: bool,
    pub bounded_inactivity_legacy_log: bool,
    pub strict_inactivity_session_evidence: bool,
    pub bounded_title_reads: bool,
    pub access_snapshot_session: bool,
    pub resource_list_batch_reads: bool,
    pub single_fence_event_pages: bool,
    pub concurrent_sandbox_detail_enrichment: bool,
    pub sandbox_agent_projection: bool,
    pub sandbox_profile_readability: bool,
    pub sandbox_snapshot_reads: bool,
    pub sandbox_batched_reads: bool,
    pub sandbox_detail_timings: bool,
    pub agent_file_read_timings: bool,
    pub bounded_response_reads: bool,
    pub active_log_proofs: bool,
    pub bounded_transcript_reads: bool,
    pub privacy_export_enabled: bool,
    pub bounded_usage_reads: bool,
    pub capacity_recovery_enabled: bool,
    pub segment_adoption_enabled: bool,
    pub paged_response_events: bool,
    pub v2_ui_transport: bool,
    pub trigger_command_admission: bool,
    pub github_repository_preflight: bool,
    pub github_connection_approval: bool,
    pub github_connection_recovery: bool,
    pub nyxid_resource_request: bool,
    pub nyxid_proxy: bool,
    /// Gated writer for the stable proxy replay principal (reader floor landed first).
    pub nyxid_proxy_replay_principal: bool,
    pub service_authorization: bool,
    pub silent_sign_in: bool,
    pub document_revocation_reserve: bool,
    pub document_quotas: bool,
    pub document_source_commits: bool,
    pub document_object_deletion: bool,
    pub document_deletion: bool,
    pub document_reclamation: bool,
    pub bounded_approval_reads: bool,
    pub bounded_close_retries: bool,
    pub retry_single_marker_enabled: bool,
    pub work_retry_policy_enabled: bool,
    pub native_create_retry_enabled: bool,
    pub reopen_output_refusal_enabled: bool,
    pub cleanup_continuation_enabled: bool,
    pub owner_stop_settlement_enabled: bool,
    pub owner_agent_restart_enabled: bool,
    pub agent_log_scan_cooperative_enabled: bool,
    pub retained_context_lookup: bool,
    pub context_diagnostics: bool,
    pub event_driven_reconciliation: bool,
    pub resume_agent_reconciliation: bool,
    pub stale_turn_reconciliation: bool,
    pub stale_turn_terminal: bool,
    pub agent_connection_observations: bool,
    pub empty_discovery: bool,
    pub sandbox_reconciliation_reserved_lane: bool,
    pub sandbox_binding_hold_metrics: bool,
    pub startup_native_home_hold_enabled: bool,
}
impl FeatureFlags {
    pub(crate) fn from_lookup(
        lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
    ) -> Result<Self, String> {
        let tail = rollout_tail::read(lookup)?;
        let flags = Self {
            binding_diagnostics: flag(lookup, "CHRONO_BINDING_DIAGNOSTICS_ENABLED")?,
            session_guidance_v4: flag(lookup, "CHRONO_AGENT_SESSION_GUIDANCE_V4_ENABLED")?,
            session_guidance_v5: flag(lookup, "CHRONO_AGENT_SESSION_GUIDANCE_V5_ENABLED")?,
            session_guidance_v6: flag(lookup, "CHRONO_AGENT_SESSION_GUIDANCE_V6_ENABLED")?,
            image_manager_retirement: flag(lookup, "CHRONO_IMAGE_MANAGER_RETIREMENT_ENABLED")?,
            am_startup_health_serving: flag(lookup, "CHRONO_AM_STARTUP_HEALTH_SERVING_ENABLED")?,
            am_empty_warm_pool_startup: flag(lookup, "CHRONO_AM_EMPTY_WARM_POOL_STARTUP_ENABLED")?,
            pending_home_reconciliation: flag(
                lookup,
                "CHRONO_PENDING_HOME_RECONCILIATION_ENABLED",
            )?,
            visitor_launch_admission: flag(lookup, "CHRONO_VISITOR_LAUNCH_ADMISSION_ENABLED")?,
            usage_signed_cursors_enabled: flag(lookup, "CHRONO_USAGE_SIGNED_CURSORS_ENABLED")?,
            usage_snapshot_cursors_enabled: flag(lookup, "CHRONO_USAGE_SNAPSHOT_CURSORS_ENABLED")?,
            usage_strict_cursors_enabled: flag(lookup, "CHRONO_USAGE_STRICT_CURSORS_ENABLED")?,
            visitor_two_sandbox_defaults: flag(
                lookup,
                "CHRONO_VISITOR_TWO_SANDBOX_DEFAULTS_ENABLED",
            )?,
            admin_runtime_inventory: flag(lookup, "CHRONO_ADMIN_RUNTIME_INVENTORY_ENABLED")?,
            agent_inspection_parking: flag(lookup, "CHRONO_AGENT_INSPECTION_PARKING_ENABLED")?,
            ownership_retry_hints_enabled: flag(
                lookup,
                "CHRONO_AGENT_OWNERSHIP_RETRY_HINT_ENABLED",
            )?,
            home_backup_enabled: flag(lookup, "CHRONO_HOME_BACKUP_ENABLED")?,
            home_backup_expiry_enabled: flag(lookup, "CHRONO_HOME_BACKUP_EXPIRY_ENABLED")?,
            bot_effect_reservations: flag(lookup, "CHRONO_BOT_EFFECT_RESERVATIONS_ENABLED")?,
            agent_model_discovery_cache: flag(
                lookup,
                "CHRONO_AGENT_MODEL_DISCOVERY_CACHE_ENABLED",
            )?,
            agent_environment_async: flag(lookup, "CHRONO_AGENT_ENVIRONMENT_ASYNC_ENABLED")?,
            agent_settings_owner_relay: flag(lookup, "CHRONO_AGENT_SETTINGS_OWNER_RELAY_ENABLED")?,
            agent_file_write_handoff: flag(lookup, "CHRONO_AGENT_FILE_WRITE_HANDOFF_ENABLED")?,
            agent_shared_observation: flag(lookup, "CHRONO_AGENT_SHARED_OBSERVATION_ENABLED")?,
            agui_bounded_replay: flag(lookup, "CHRONO_AGUI_BOUNDED_REPLAY_ENABLED")?,
            agent_event_stream_wake: flag(lookup, "CHRONO_AGENT_EVENT_STREAM_WAKE_ENABLED")?,
            agent_event_stream_fallback: super::public_streams::event_fallback(lookup)?,
            runtime_loss_bounded_proof_enabled: flag(
                lookup,
                "CHRONO_RUNTIME_LOSS_BOUNDED_PROOF_ENABLED",
            )?,
            native_retirement_finality_enabled: flag(
                lookup,
                "CHRONO_NATIVE_RETIREMENT_FINALITY_ENABLED",
            )?,
            native_retirement_capture_enabled: flag(lookup, gates::RETIREMENT_CAPTURE)?,
            reviewed_sandbox_retirement_enabled: flag(
                lookup,
                "CHRONO_REVIEWED_SANDBOX_RETIREMENT_ENABLED",
            )?,
            command_reconcile_pacing: flag(lookup, "CHRONO_COMMAND_RECONCILE_PACING_ENABLED")?,
            command_cancel_wake: flag(lookup, "CHRONO_COMMAND_CANCEL_WAKE_ENABLED")?,
            ownership_cleanup_pacing: flag(lookup, "CHRONO_OWNERSHIP_CLEANUP_PACING_ENABLED")?,
            batch_ownership_renewal: flag(lookup, "CHRONO_AM_BATCH_OWNERSHIP_RENEWAL")?,
            am_failure_containment_enabled: flag(
                lookup,
                "CHRONO_AGENT_MANAGER_FAILURE_CONTAINMENT_ENABLED",
            )?,
            receiver_housekeeping_containment: flag(
                lookup,
                "CHRONO_RECEIVER_HOUSEKEEPING_CONTAINMENT_ENABLED",
            )?,
            coordinated_ingress: flag(lookup, "CHRONO_INGRESS_COORDINATOR_ENABLED")?,
            retained_json: flag(lookup, "CHRONO_RUNTIME_JSON_RETENTION_BOUNDS_ENABLED")?,
            bounded_raw_output: flag(lookup, "CHRONO_RUNTIME_RAW_OUTPUT_BOUNDS_ENABLED")?,
            output_json_budget: flag(lookup, "CHRONO_RUNTIME_OUTPUT_JSON_BUDGET_ENABLED")?,
            bounded_update_relay: flag(lookup, "CHRONO_RUNTIME_UPDATE_RELAY_BOUNDED_ENABLED")?,
            native_close_agent_lane: flag(lookup, "CHRONO_NATIVE_CLOSE_AGENT_LANE_ENABLED")?,
            bounded_live_fragments: flag(lookup, "CHRONO_AGENT_LIVE_BUFFER_BUDGET_ENABLED")?,
            quiet_output: super::quiet_output::from_lookup(lookup)?,
            retain_read_frames: flag(lookup, "CHRONO_READ_FRAME_BUDGET_ENABLED")?,
            bounded_permission_state: flag(lookup, "CHRONO_RUNTIME_PERMISSION_BOUNDS_ENABLED")?,
            parent_not_ready_pacing_enabled: flag(
                lookup,
                "CHRONO_AGENT_PARENT_NOT_READY_PACING_ENABLED",
            )?,
            startup_sibling_hold_enabled: flag(
                lookup,
                "CHRONO_AGENT_STARTUP_SIBLING_HOLD_ENABLED",
            )?,
            retained_reopen_enabled: flag(lookup, "CHRONO_RETAINED_REOPEN_ENABLED")?,
            runtime_image_origin_enabled: flag(lookup, "CHRONO_RUNTIME_IMAGE_ORIGIN_ENABLED")?,
            managed_machine_updates: ManagedMachineUpdates::read(lookup)?,
            retained_suspended_reopen_enabled: flag(
                lookup,
                "CHRONO_RETAINED_SUSPENDED_REOPEN_ENABLED",
            )?,
            retained_repeat_reopen_enabled: flag(lookup, "CHRONO_RETAINED_REPEAT_REOPEN_ENABLED")?,
            agent_skill_upgrade_enabled: flag(lookup, "CHRONO_AGENT_SKILL_UPGRADE_ENABLED")?,
            access_snapshot_session: super::access_snapshot::enabled(lookup)?,
            resource_list_batch_reads: flag(lookup, "CHRONO_RESOURCE_LIST_BATCH_READS_ENABLED")?,
            single_fence_event_pages: flag(lookup, "CHRONO_AGENT_EVENT_PAGE_SINGLE_FENCE_ENABLED")?,
            concurrent_sandbox_detail_enrichment: flag(
                lookup,
                "CHRONO_SANDBOX_DETAIL_ENRICHMENT_CONCURRENT_ENABLED",
            )?,
            agent_file_read_timings: flag(lookup, "CHRONO_AGENT_FILE_READ_TIMINGS_ENABLED")?,
            sandbox_agent_projection: flag(lookup, "CHRONO_SANDBOX_AGENT_PROJECTION_ENABLED")?,
            sandbox_profile_readability: flag(
                lookup,
                "CHRONO_SANDBOX_PROFILE_READABILITY_ENABLED",
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
            document_source_commits: flag(lookup, "CHRONO_DOCUMENT_SOURCE_COMMITS_ENABLED")?,
            document_object_deletion: flag(lookup, "CHRONO_DOCUMENT_OBJECT_DELETION_ENABLED")?,
            document_deletion: flag(lookup, "CHRONO_DOCUMENT_DELETION_ENABLED")?,
            document_reclamation: flag(lookup, "CHRONO_DOCUMENT_RECLAIM_ENABLED")?,
            shared_runtime_output: flag(lookup, "CHRONO_RUNTIME_SHARED_OUTPUT_ENABLED")?,
            bounded_close_retries: flag(lookup, "CHRONO_AGENT_CLOSE_RETRY_BOUNDED_ENABLED")?,
            retry_single_marker_enabled: flag(lookup, "CHRONO_AGENT_RETRY_SINGLE_MARKER_ENABLED")?,
            work_retry_policy_enabled: flag(lookup, "CHRONO_WORK_RETRY_POLICY_ENABLED")?,
            native_create_retry_enabled: flag(
                lookup,
                "CHRONO_SANDBOX_NATIVE_CREATE_RETRY_ENABLED",
            )?,
            reopen_output_refusal_enabled: flag(
                lookup,
                "CHRONO_RUNTIME_UPDATE_REOPEN_OUTPUT_REFUSAL_ENABLED",
            )?,
            cleanup_continuation_enabled: flag(
                lookup,
                "CHRONO_AGENT_CLEANUP_CONTINUATION_ENABLED",
            )?,
            owner_agent_restart_enabled: flag(lookup, "CHRONO_OWNER_AGENT_RESTART_ENABLED")?,
            owner_stop_settlement_enabled: flag(
                lookup,
                "CHRONO_AGENT_OWNER_STOP_SETTLEMENT_ENABLED",
            )?,
            agent_log_scan_cooperative_enabled: flag(
                lookup,
                "CHRONO_AGENT_LOG_SCAN_COOPERATIVE_ENABLED",
            )?,
            retained_context_lookup: flag(lookup, "CHRONO_CMAEG_RETAINED_CONTEXT_LOOKUP_ENABLED")?,
            context_diagnostics: flag(lookup, "CHRONO_CMAEG_CONTEXT_DIAGNOSTICS_ENABLED")?,
            event_driven_reconciliation: flag(
                lookup,
                "CHRONO_AGENT_RECONCILIATION_EVENT_DRIVEN_ENABLED",
            )?,
            resume_agent_reconciliation: flag(
                lookup,
                "CHRONO_AGENT_RECONCILIATION_RESUME_ENABLED",
            )?,
            empty_discovery: flag(lookup, "CHRONO_EMPTY_DISCOVERY_COALESCING_ENABLED")?,
            agent_connection_observations: flag(
                lookup,
                "CHRONO_AGENT_RECONCILIATION_VALIDATION_REUSE_ENABLED",
            )?,
            sandbox_reconciliation_reserved_lane: flag(
                lookup,
                "CHRONO_SANDBOX_RECONCILIATION_RESERVED_LANE_ENABLED",
            )?,
            startup_native_home_hold_enabled: flag(
                lookup,
                "CHRONO_STARTUP_NATIVE_HOME_HOLD_ENABLED",
            )?,
            sandbox_binding_hold_metrics: flag(
                lookup,
                "CHRONO_SANDBOX_BINDING_HOLD_METRICS_ENABLED",
            )?,
            stale_turn_reconciliation: flag(lookup, "CHRONO_STALE_TURN_RECONCILIATION_ENABLED")?,
            stale_turn_terminal: flag(lookup, "CHRONO_STALE_TURN_TERMINAL_ENABLED")?,
            bounded_approval_reads: flag(lookup, "CHRONO_AGENT_APPROVAL_BOUNDED_READS_ENABLED")?,
            model_budget_writes_enabled: flag(lookup, "CHRONO_MODEL_BUDGET_WRITES_ENABLED")?,
            model_credit_required: flag(lookup, "CHRONO_AGENT_MODEL_CREDIT_REQUIRED")?,
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
            admission_ceilings_writes_enabled: flag(
                lookup,
                "CHRONO_ADMISSION_CEILINGS_WRITES_ENABLED",
            )?,
            agent_control_write_coordination: flag(
                lookup,
                "CHRONO_AGENT_CONTROL_WRITE_COORDINATION_ENABLED",
            )?,
            bounded_inactivity_legacy_log: flag(
                lookup,
                "CHRONO_BOUNDED_INACTIVITY_LEGACY_LOG_ENABLED",
            )?,
            strict_inactivity_session_evidence: flag(
                lookup,
                "CHRONO_STRICT_INACTIVITY_SESSION_EVIDENCE_ENABLED",
            )?,
            bounded_title_reads: flag(lookup, "CHRONO_AGENT_TITLE_BOUNDED_READS_ENABLED")?,
            bounded_response_reads: flag(lookup, "CHRONO_AGENT_RESPONSE_BOUNDED_READS_ENABLED")?,
            active_log_proofs: flag(lookup, "CHRONO_AGENT_LOG_ACTIVE_PROOFS_ENABLED")?,
            bounded_transcript_reads: flag(
                lookup,
                "CHRONO_AGENT_TRANSCRIPT_BOUNDED_READS_ENABLED",
            )?,
            privacy_export_enabled: flag(lookup, "CHRONO_ACCOUNT_PRIVACY_EXPORT_ENABLED")?,
            bounded_usage_reads: flag(lookup, "CHRONO_AGENT_USAGE_BOUNDED_READS_ENABLED")?,
            capacity_recovery_enabled: flag(lookup, "CHRONO_AGENT_CAPACITY_RECOVERY_ENABLED")?,
            segment_adoption_enabled: tail.segment_adoption_enabled,
            paged_response_events: tail.paged_response_events,
            v2_ui_transport: tail.v2_ui_transport,
            trigger_command_admission: tail.trigger_command_admission,
            github_repository_preflight: tail.github_repository_preflight,
            github_connection_approval: tail.github_connection_approval,
            github_connection_recovery: tail.github_connection_recovery,
            nyxid_resource_request: tail.nyxid_resource_request,
            nyxid_proxy: tail.nyxid_proxy,
            nyxid_proxy_replay_principal: tail.nyxid_proxy_replay_principal,
            service_authorization: tail.service_authorization,
            silent_sign_in: tail.silent_sign_in,
            document_revocation_reserve: tail.document_revocation_reserve,
        };
        usage_cursors::validate(&flags)?;
        if flags.agent_shared_observation && !flags.agui_bounded_replay {
            return Err("CHRONO_AGENT_SHARED_OBSERVATION_ENABLED requires CHRONO_AGUI_BOUNDED_REPLAY_ENABLED".into());
        }
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
        if flags.am_startup_health_serving && flags.am_failure_containment_enabled {
            return Err("CHRONO_AM_STARTUP_HEALTH_SERVING_ENABLED cannot be combined with CHRONO_AGENT_MANAGER_FAILURE_CONTAINMENT_ENABLED".into());
        }
        if flags.am_empty_warm_pool_startup && !flags.am_startup_health_serving {
            return Err("CHRONO_AM_EMPTY_WARM_POOL_STARTUP_ENABLED requires CHRONO_AM_STARTUP_HEALTH_SERVING_ENABLED".into());
        }
        Ok(flags)
    }
}

#[cfg(test)]
#[path = "admission_control_tests.rs"]
mod admission_control_tests;
#[cfg(test)]
mod approval_tests;
#[cfg(test)]
mod capacity_tests;
#[cfg(test)]
mod close_retry_tests;
#[cfg(test)]
mod discovery_tests;
#[cfg(test)]
mod document_maintenance_tests;
#[cfg(test)]
mod live_buffer_tests;
#[cfg(test)]
mod native_create_retry_tests;
#[cfg(test)]
mod observation_tests;
#[cfg(test)]
mod ownership_retry_hint_tests;
#[cfg(test)]
mod paged_event_tests;
#[cfg(test)]
mod parent_pacing_tests;
#[cfg(test)]
mod permission_bound_tests;
#[cfg(test)]
mod raw_output_tests;
#[cfg(test)]
mod read_frame_tests;
#[cfg(test)]
mod reconciliation_tests;
#[cfg(test)]
mod relay_tests;
#[cfg(test)]
mod response_tests;
#[cfg(test)]
mod retained_json_tests;
#[cfg(test)]
mod retry_single_marker_tests;
#[cfg(test)]
mod rollout_tail_tests;
#[cfg(test)]
mod runtime_loss_tests;
#[cfg(test)]
mod sandbox_batch_tests;
#[cfg(test)]
#[path = "sandbox_enrichment_tests.rs"]
mod sandbox_enrichment_tests;
#[cfg(test)]
#[path = "sandbox_projection_tests.rs"]
mod sandbox_projection_tests;
#[cfg(test)]
#[path = "sandbox_detail_diagnostics_tests.rs"]
mod sandbox_read_tests;
#[cfg(test)]
#[path = "scan_cooperation_tests.rs"]
mod scan_cooperation_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod transcript_tests;
#[cfg(test)]
mod usage_cursor_tests;
mod usage_cursors;
#[cfg(test)]
mod work_retry_tests;

mod launch_admission;

#[cfg(test)]
mod pending_homes_tests;

#[cfg(test)]
#[path = "features/visitor_defaults_tests.rs"]
mod visitor_defaults_tests;

#[cfg(test)]
mod bot_effect_tests;
