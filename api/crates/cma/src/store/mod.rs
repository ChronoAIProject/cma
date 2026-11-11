//! Durable adapters that do not depend on a resource aggregate.

pub mod activity;
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
mod agent_control_lineage;
mod agent_control_receipt;
mod agent_control_store;
mod agent_control_transaction;
mod agent_conversation_store;
mod agent_failure_pacing;
mod agent_lifecycle_operations;
mod agent_log;
mod agent_log_codec;
mod agent_log_integrity;
mod agent_log_reads;
pub(crate) mod agent_log_segments;
mod agent_log_store;
pub(crate) mod agent_log_telemetry;
mod agent_log_transcript;
mod agent_log_usage;
mod agent_profiles;
mod agent_provenance;
mod agent_provenance_backfill;
mod agent_provenance_inventory;
mod agent_reads;
mod agent_task_status;
mod agent_title_claims;
mod agent_title_retry;
mod agent_title_state;
mod agent_title_writes;
mod agent_titles;
mod agent_worker_capacity;
mod agent_worker_reconcile;
mod agent_worker_store;
mod agent_worker_transaction;
mod agent_worktree_recovery;
mod agent_worktree_recovery_records;
mod agent_write_coordination;
mod agents;
pub mod auth;
mod bindings;
pub(crate) mod bot_setup_contexts;
pub(crate) mod bot_setups;
pub(crate) mod bots;
#[cfg(test)]
mod bots_mongo_integration_tests;
mod budget;
mod channel_storage_retention;
mod checkpoint_capture;
mod checkpoint_objects;
mod checkpoint_storage;
mod checkpoints;
mod child_writer_claims;
mod codex_subscriptions;
mod default_profiles;
pub mod directory;
mod durability;
mod encryption;
mod media_uploads;
pub(crate) mod provenance_progress;
mod segment_response;
pub(crate) use segment_response::emit as emit_response_reads;
// Retain legacy persistence contracts for migration verification; production
// Gateway records belong to the independently deployed Event Gateway.
#[cfg(test)]
mod gateway_interactions;
#[cfg(test)]
mod gateway_records;
#[cfg(test)]
mod gateway_records_mongo_integration_tests;
mod hierarchy;
mod launch_context;
#[cfg(test)]
mod launch_context_mongo_integration_tests;
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
mod message_feedback;
pub(crate) mod migrations;
pub mod mongo;
mod mongo_latency;
pub(crate) mod mongo_read_telemetry;
pub(crate) mod native_prepared;
pub mod operation;
mod operator_profiles;
mod operator_sandboxes;
mod profile_documents;
#[cfg(test)]
mod profile_index_tests;
mod profile_mongo;
#[cfg(test)]
mod profile_operator_mongo_integration_tests;
mod profile_references;
mod profiles;
pub(crate) mod request_limits;
mod resource_access;
mod retained_reopen;
mod sandbox_admission;
#[cfg(test)]
mod sandbox_admission_mongo_integration_tests;
pub(crate) mod sandbox_billing;
pub(crate) mod sandbox_colors;
mod sandbox_controls;
mod sandbox_credentials;
mod sandbox_events;
mod sandbox_maintenance;
mod sandbox_mongo;
#[cfg(test)]
mod sandbox_mongo_integration_tests;
#[cfg(test)]
mod sandbox_operator_mongo_integration_tests;
mod sandbox_profiles;
mod sandbox_reconciliation;
mod sandbox_sessions;
mod sandbox_transactions;
mod sandbox_unpublished_recovery;
pub(crate) mod sandbox_usage;
mod sandbox_worker_store;
mod sandboxes;
#[cfg(test)]
mod sandboxes_tests;
mod schema;
#[cfg(test)]
mod schema_mongo_integration_tests;
pub(crate) mod segment_operations;
pub mod settings;
mod shared_child_capacity;
mod suggestions;
mod trigger_definitions;
mod trigger_first_messages;
mod warm_sandboxes;
pub(crate) use native_prepared::{NativePreparedClaim, NativePreparedPlan};
mod workspace_admission;
mod workspace_prepare_recovery;
mod workspace_profiles;
mod workspace_removal;
mod workspace_worker_store;
mod workspaces;
mod worktrees;

pub use activity::MongoActivityStore;
#[cfg(test)]
pub(crate) use agent_access_tools::fixtures as agent_access_fixtures;
pub(crate) use agent_access_tools::MongoAgentAccessToolSource;
#[cfg(test)]
pub(crate) use agent_lifecycle_operations::LifecycleRecord;
pub(crate) use agent_lifecycle_operations::{
    CreateResult, LifecycleBehavior, LifecycleInput, LifecycleLease, MongoAgentLifecycleStore,
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
pub(crate) use sandbox_credentials::MongoSandboxCredentialStore;
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

// Measured reporting read models (#1332); independent of lifecycle writes.
pub(crate) mod sandbox_metric_bindings;
pub(crate) mod sandbox_metric_rollups;
pub(crate) mod sandbox_usage_rollups;
pub(crate) mod usage_reports;
