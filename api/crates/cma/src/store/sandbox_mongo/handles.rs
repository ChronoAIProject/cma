//! Collection construction without serving initialization or database writes.
use super::*;
use crate::store;

impl MongoSandboxStore {
    // Collection handles only. Kept separate so read-only audits never invoke
    // serving initialization, settings writes, index DDL or effect workers.
    pub(super) fn handles(
        client: &Client,
        name: &str,
        checkpoint_objects: MongoCheckpointObjectStore,
        bindings: store::bindings::BindingCollections,
        initial_settings: store::settings_defaults::InitialSettings,
    ) -> Self {
        let database = client.database(name);
        Self {
            creation_writer_epoch: None,
            usage_capture: None,
            initial_settings,
            empty_discovery: Default::default(),
            agent_connection_observations: None,
            segment_logs: store::agent_log_segments::SegmentCollections::new(&database),
            global_sandbox_limit: 0,
            public_owner_sandbox_limit: None,
            #[cfg(test)]
            request_budget_clock: None,
            bounded_admission_agents: Default::default(),
            runtime_migration_authority: None,
            runtime_migration_verifier: None,
            runtime_migration_enabled: false,
            renewable_work: false,
            segment_adoption_enabled: false,
            bounded_approval_reads: false,
            runtime_loss_bounded_proof_enabled: false,
            bounded_close_retries: false,
            retry_single_marker_enabled: false,
            retry_audits: Default::default(),
            scan_cooperation: Default::default(),
            fleet_log_activation: Default::default(),
            platform_counter_shards: false,
            bounded_title_reads: false,
            title_effect_custody: false,
            bounded_transcript_reads: false,
            bounded_usage_reads: false,
            access_snapshot_session: false,
            resource_list_batch_reads: false,
            sandbox_agent_projection: false,
            sandbox_snapshot_reads: false,
            sandbox_batched_reads: false,
            bounded_response_reads: false,
            active_log_proofs: false,
            writer_policy: Default::default(),
            bindings,
            client: client.clone(),
            checkpoint_objects,
            sandboxes: majority_collection(&database, SANDBOXES),
            lifecycle_operations: majority_collection(
                &database,
                store::agent_lifecycle_operations::COLLECTION,
            ),
            sandbox_sessions: majority_collection(&database, SANDBOX_SESSIONS),
            strict_inactivity_session_evidence: false,
            workspaces: majority_collection(&database, WORKSPACES),
            bounded_inactivity_legacy_log: false,
            inactivity_log_preparation: std::sync::Arc::new(tokio::sync::Semaphore::new(1)),
            worktrees: majority_collection(&database, WORKTREES),
            agents: majority_collection(&database, AGENTS),
            agent_writes: Default::default(),
            agent_control_write_coordination: false,
            agent_ownership: None,
            agent_inspection_parking: false,
            ownership_retry_hints_enabled: false,
            #[cfg(test)]
            ownership_retry_hint_now: None,
            agent_manager_instance: None,
            platform: Default::default(),
            agent_access_tools: majority_collection(
                &database,
                store::agent_access_tools::COLLECTION,
            ),
            agent_access_requests: majority_collection(
                &database,
                store::agent_access_requests::COLLECTION,
            ),
            agent_reuse_bindings: majority_collection(&database, AGENT_REUSE_BINDINGS),
            agent_log: majority_collection(&database, AGENT_LOG),
            suggestions: majority_collection(&database, "agent_suggestions"),
            suggestion_control: None,
            agent_creation_provenance: majority_collection(&database, "agent_creation_provenance"),
            agent_channel_usage: majority_collection(&database, "agent_channel_usage"),
            channel_usage_events: majority_collection(&database, "channel_usage_events"),
            agent_commands: majority_collection(&database, AGENT_COMMANDS),
            model_budget: majority_collection(&database, store::budget::COLLECTION),
            budget_settings: majority_collection(&database, "platform_settings"),
            agent_artifacts: majority_collection(&database, AGENT_ARTIFACTS),
            media_uploads: majority_collection(&database, store::media_uploads::COLLECTION),
            sandbox_controls: majority_collection(&database, SANDBOX_CONTROLS),
            command_authorization_audits: majority_collection(
                &database,
                COMMAND_AUTHORIZATION_AUDITS,
            ),
            agent_command_orders: majority_collection(&database, AGENT_COMMAND_ORDERS),
            resource_policies: majority_collection(&database, RESOURCE_POLICIES),
            resource_share_commands: majority_collection(&database, RESOURCE_SHARE_COMMANDS),
            resource_shares: majority_collection(&database, RESOURCE_SHARES),
            sandbox_events: majority_collection(&database, SANDBOX_EVENTS),
            sandbox_usage: majority_collection(&database, store::sandbox_usage::STATES),
            sandbox_usage_events: majority_collection(&database, store::sandbox_usage::EVENTS),
            sandbox_billing: majority_collection(&database, SANDBOX_BILLING),
            sandbox_admissions: majority_collection(&database, SANDBOX_ADMISSIONS),
            checkpoints: majority_collection(&database, CHECKPOINTS),
            sandbox_profiles: majority_collection(&database, SANDBOX_PROFILES),
            #[cfg(test)]
            sandbox_profile_revisions: majority_collection(&database, SANDBOX_PROFILE_REVISIONS),
            #[cfg(test)]
            warm_workspace_revisions: majority_collection(&database, WORKSPACE_PROFILE_REVISIONS),
            #[cfg(test)]
            warm_agent_revisions: majority_collection(&database, AGENT_PROFILE_REVISIONS),
            warm_sandboxes: majority_collection(&database, WARM_SANDBOXES),
            native_prepared: majority_collection(&database, store::native_prepared::COLLECTION),
            warm_runtime_version: None,
            managed_runtime_version: None,
            default_pool_holds: Default::default(),
            runtime_image_origin_enabled: false,
            managed_machine_updates: Default::default(),
            desktop_verified_runtime_image: None,
            released_lanes: None,
            managed_machine_runtime_version: None,
            released: None,
            warm_home_policy: None,
            warmup_changed: std::sync::Arc::new(tokio::sync::Notify::new()),
            native_prepared_changed: std::sync::Arc::new(tokio::sync::Notify::new()),
            sandbox_admitted: std::sync::Arc::new(tokio::sync::Notify::new()),
            access_admitted: std::sync::Arc::new(tokio::sync::Notify::new()),
            agent_admitted: std::sync::Arc::new(tokio::sync::Notify::new()),
            agent_parent_ready: std::sync::Arc::new(tokio::sync::Notify::new()),
        }
    }

    pub(in crate::store) fn audit_handles(client: &Client, name: &str) -> Self {
        let database = client.database(name);
        Self::handles(
            client,
            name,
            MongoCheckpointObjectStore::handles(
                client,
                name,
                std::sync::Arc::new(store::checkpoint_storage::RetiredCheckpointStorage),
            ),
            store::bindings::BindingCollections::handles(&database, Default::default()),
            Default::default(),
        )
    }
}
