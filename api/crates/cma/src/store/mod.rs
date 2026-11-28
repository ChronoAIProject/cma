//! Durable adapters that do not depend on a resource aggregate.
mod admission_capacity;
mod admission_checks;

pub(crate) mod account_deletions;
pub(crate) mod account_erasure;
pub mod activity;
pub(crate) mod admission_ceilings;
pub(crate) mod admission_control;
#[cfg(test)]
mod admission_wake_test_support;
pub(crate) mod agent_access_requests;
mod agent_access_tools;
mod agent_admission;
mod agent_admission_records;
pub(crate) mod agent_capacity_recovery;
mod agent_capacity_recovery_presence;
mod agent_channel_usage;
mod agent_commands;
mod agent_connection_observations;
mod agent_control_lineage;
mod agent_control_receipt;
mod agent_control_store;
mod agent_control_transaction;
mod agent_conversation_store;
mod agent_failure_pacing;
pub(crate) mod agent_inspection_park;
mod agent_lifecycle_operations;
pub(crate) use agent_lifecycle_operations::released::managed_lifecycle_release_lane;
mod agent_log;
mod agent_log_codec;
mod agent_log_integrity;
mod agent_log_reads;
pub(crate) mod agent_log_segment_reads;
pub(crate) mod agent_log_segments;
mod agent_log_store;
pub(crate) mod agent_log_telemetry;
mod agent_log_transcript;
mod agent_log_usage;
pub(crate) mod agent_observation;
mod agent_opening_reads;
pub(crate) mod agent_ownership;
mod agent_profiles;
mod agent_provenance;
mod agent_provenance_backfill;
mod agent_provenance_inventory;
mod agent_reads;
mod agent_task_status;
mod agent_title_claims;
mod agent_title_effect;
mod agent_title_retry;
mod agent_title_state;
mod agent_title_writes;
mod agent_titles;
mod agent_tool_open;
mod agent_worker_capacity;
mod agent_worker_reconcile;
mod agent_worker_store;
mod agent_worker_transaction;
pub(crate) mod stale_turns;
pub(crate) mod transaction_barrier;
pub(crate) mod turn_timeouts;
#[cfg(test)]
pub(crate) use agent_worker_transaction::adoption_probe;
#[cfg(test)]
pub(crate) use agent_worker_transaction::skill_result_test_support;
mod agent_worktree_recovery;
mod agent_worktree_recovery_records;
mod agent_write_coordination;
pub(crate) use agent_write_coordination::CLEANUP_RETRY_DEADLINE;
mod agents;
pub mod auth;
mod billing_authority;
mod bindings;
pub(crate) mod bot_setup_contexts;
pub(crate) mod bot_setups;
pub(crate) mod bots;
#[cfg(test)]
mod bots_mongo_integration_tests;
mod budget;
pub(crate) use budget::inventory as turn_budget_inventory;
#[cfg(test)]
pub(crate) mod budget_test_fixture;
mod channel_storage_retention;
mod checkpoint_capture;
mod checkpoint_objects;
mod checkpoint_storage;
mod checkpoints;
mod child_writer_claims;
mod codex_subscriptions;
mod connection_recovery_activity;
mod contention;
mod cooperative_budget;
pub(crate) mod creation_outcomes;
mod default_profiles;
pub mod directory;
mod durability;
mod empty_discovery;
mod encryption;
mod execution_sections;
pub(crate) mod legal_consents;
mod media_uploads;
mod online_backfill;
pub(crate) mod provenance_progress;
pub(crate) mod registration_capacity;
mod retained_consents;
mod segment_response;
mod transaction_scope;
pub(crate) mod transaction_terminal;
pub(crate) use segment_response::emit as emit_response_reads;

pub(crate) async fn backfill_progress_command(
    arguments: &[String],
) -> Option<Result<serde_json::Value, &'static str>> {
    if online_backfill::is_command(arguments) {
        Some(online_backfill::command(arguments).await)
    } else if provenance_progress::is_command(arguments) {
        Some(provenance_progress::command(arguments).await)
    } else {
        None
    }
}

// Retain legacy persistence contracts for migration verification; production
// Gateway records belong to the independently deployed Event Gateway.
#[cfg(test)]
mod gateway_interactions;
#[cfg(test)]
mod gateway_records;
#[cfg(test)]
mod gateway_records_mongo_integration_tests;
mod hierarchy;
#[allow(dead_code)] // Foundation primitives have no production capture worker.
pub(crate) mod home_backups;
mod launch_context;
#[cfg(test)]
mod launch_context_mongo_integration_tests;
pub(crate) mod launch_home_capacity;
pub(crate) mod ledger_census;
pub(crate) mod ledger_collection;
mod ledger_collection_progress;
pub(crate) mod ledger_retirement;
mod legacy_history;
mod lifecycle_execution;
pub(crate) mod lifecycle_transaction;
mod marketplace;
#[cfg(test)]
mod marketplace_mongo_integration_tests;
mod marketplace_mutations;
mod marketplace_query;
mod marketplace_support;
mod marketplace_visibility;
pub(crate) mod measured_capture;
mod message_feedback;
pub(crate) mod migrations;
pub mod mongo;
mod mongo_diagnostics;
pub(crate) mod pending_launch_homes;
#[cfg(test)]
pub(crate) mod test_visibility;
#[cfg(test)]
pub(crate) use mongo_diagnostics::attach_readiness_fixture;
mod managed_machine_origin;
pub(crate) mod mongo_latency;
pub(crate) mod mongo_read_telemetry;
pub(crate) mod native_prepared;
pub(crate) mod native_retirement_action;
pub mod operation;
mod operator_profiles;
mod operator_sandboxes;
mod platform_write_coordination;
mod profile_documents;
mod profile_draft_admission;
#[cfg(test)]
mod profile_index_tests;
#[cfg(test)]
mod profile_initial_publication_mongo_tests;
mod profile_mongo;
#[cfg(test)]
mod profile_operator_mongo_integration_tests;
mod profile_pin_admission;
mod profile_references;
mod profiles;
pub(crate) mod request_limits;
mod resource_access;
mod retained_reopen;
pub(crate) mod retirement_archive;
mod sandbox_admission;
#[cfg(test)]
pub(crate) mod sandbox_admission_mongo_integration_tests;
pub(crate) mod sandbox_billing;
pub(crate) mod sandbox_colors;
mod sandbox_controls;
mod sandbox_credentials;
mod sandbox_events;
pub(crate) mod sandbox_maintenance;
mod sandbox_mongo;
#[cfg(test)]
mod sandbox_mongo_integration_tests;
mod snapshot_collection;
#[cfg(test)]
pub(crate) mod test_measurement;
#[cfg(test)]
mod worker_runtime_supervision_tests;
#[cfg(test)]
pub(crate) use sandbox_mongo_integration_tests::capacity_fixtures::fill_normal_capacity as fill_agent_capacity_fixture;
#[cfg(test)]
pub(crate) const AGENT_STOP_CAPACITY_BYTES: usize = agent_log_integrity::STOP_AGENT_LOG_BYTES;
#[cfg(test)]
pub(crate) use sandbox_mongo_integration_tests::capacity_fixtures::fill_to as fill_agent_capacity_to_fixture;
#[cfg(test)]
pub(crate) use sandbox_mongo_integration_tests::capacity_fixtures::fingerprint as agent_capacity_fixture_fingerprint;
#[cfg(test)]
mod sandbox_operator_mongo_integration_tests;
mod sandbox_profiles;
mod sandbox_reconciliation;
mod sandbox_sessions;
mod sandbox_transactions;
mod sandbox_unpublished_recovery;
pub(crate) mod sandbox_usage;
pub(crate) mod sandbox_usage_export;
mod sandbox_worker_store;
mod sandboxes;
#[cfg(test)]
mod sandboxes_tests;
mod schema;
#[cfg(test)]
mod schema_mongo_integration_tests;
pub(crate) mod segment_operations;
pub mod settings;
mod settings_barrier;
pub(crate) mod settings_defaults;
mod shared_child_capacity;
pub(crate) mod suggestion_control;
mod suggestions;
pub(crate) mod task_audit;
mod trigger_definitions;
mod trigger_first_messages;
mod warm_sandboxes;
pub(crate) mod worker_observations;
pub(crate) use native_prepared::{NativePreparedClaim, NativePreparedPlan};
mod work_claim_authority;
mod work_lease_parent;
mod work_lease_renewal;
mod work_lease_update;
mod work_publication;
mod work_schema;
mod workspace_admission;
mod workspace_prepare_recovery;
mod workspace_profiles;
mod workspace_removal;
mod workspace_worker_store;
mod workspaces;
mod worktrees;

pub use activity::MongoActivityStore;
pub use admission_ceilings::MongoAdmissionCeilingsStore;
pub use admission_control::MongoAdmissionControlStore;
#[cfg(test)]
pub(crate) use agent_access_tools::fixtures as agent_access_fixtures;
pub(crate) use agent_access_tools::MongoAgentAccessToolSource;
pub(crate) use agent_lifecycle_operations::canary_admission as runtime_canary;
pub(crate) use agent_lifecycle_operations::canary_key;
#[cfg(test)]
pub(crate) use agent_lifecycle_operations::LifecycleRecord;
pub(crate) use agent_lifecycle_operations::{
    CanaryProfiles, CreateResult, LifecycleBehavior, LifecycleCreateError, LifecycleInput,
    LifecycleLease, MongoAgentLifecycleStore, PublicRepositoryPin, RepositoryAccessPin,
    DISCOVERY_BATCH, HEARTBEAT,
};
pub use auth::MongoAuthStore;
pub(crate) use bindings::{MongoBindingCredentials, MongoProviderToolSource};
pub use checkpoint_objects::MongoCheckpointObjectStore;
pub(crate) use checkpoint_storage::CheckpointBlobStore;
pub(crate) use codex_subscriptions::MongoSubscriptionStore;
pub(crate) use default_profiles::DefaultProfileSpec;
pub use directory::MongoUserDirectoryStore;
pub(crate) use encryption::AuthEncryptionKey;
pub use mongo::MongoStores;
pub use operation::MongoOperationStore;
pub use registration_capacity::MongoRegistrationCapacityStore;
pub(crate) use sandbox_credentials::MongoSandboxCredentialStore;
pub(crate) use sandbox_mongo::{
    commit_transaction, is_confirmed_body_retry, is_duplicate_key, start_transaction, CommitOutcome,
};
pub use sandbox_mongo::{MongoProfileStore, MongoSandboxStore};
pub use settings::MongoPlatformSettingsStore;
pub use trigger_definitions::MongoTriggerDefinitionStore;
pub(crate) use warm_sandboxes::WarmSandboxClaim;

pub(crate) mod event_workers;
pub(crate) mod isolation;
mod legacy_finalization;
mod legacy_reuse_lifecycle;
mod legacy_sandbox;
pub(crate) mod runtime_routing;

pub(crate) mod native_warm_pools;
pub(crate) mod released_runtimes;

// Measured reporting read models (#1332); independent of lifecycle writes.
pub(crate) mod allocation_buckets;
pub(crate) mod sandbox_metric_bindings;
pub(crate) mod sandbox_metric_rollups;
pub(crate) mod sandbox_usage_rollups;
pub(crate) mod usage_reports;

#[cfg(test)]
pub(crate) async fn open_pointer_fixture_sandbox_store(
    database: &mongodb::Database,
    pin: String,
    released: std::sync::Arc<released_runtimes::StoreReleasedRuntime>,
) -> Result<MongoSandboxStore, layer2_core::StoreError> {
    let mut store = MongoSandboxStore::open(database.client(), database.name())
        .await?
        .with_managed_runtime_version(pin);
    store.released_lanes = Some(released.lanes.clone());
    Ok(store.with_released_runtime(released))
}

#[cfg(test)]
pub(crate) async fn open_pointer_fixture_pool_store(
    database: &mongodb::Database,
    deployment: &str,
) -> Result<native_warm_pools::MongoNativeWarmPools, layer2_core::StoreError> {
    let mut store = native_warm_pools::MongoNativeWarmPools::open(database, deployment).await?;
    store.released_lanes = Some(released_runtimes::MongoReleasedRuntimes::open(
        database, deployment,
    ));
    Ok(store)
}

#[cfg(test)]
pub(crate) async fn open_pointer_fixture_lifecycle_store(
    database: &mongodb::Database,
    released: &released_runtimes::StoreReleasedRuntime,
) -> Result<MongoAgentLifecycleStore, layer2_core::StoreError> {
    let mut store = MongoAgentLifecycleStore::open(database).await?;
    store.released_lanes = Some(released.lanes.clone());
    Ok(store)
}
