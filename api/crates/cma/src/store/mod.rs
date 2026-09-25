//! Durable adapters that do not depend on a resource aggregate.

pub mod activity;
mod agent_admission;
mod agent_admission_records;
mod agent_channel_usage;
mod agent_commands;
mod agent_control_lineage;
mod agent_control_receipt;
mod agent_control_store;
mod agent_control_transaction;
mod agent_conversation_store;
mod agent_log;
mod agent_log_integrity;
mod agent_profiles;
mod agent_provenance;
mod agent_provenance_backfill;
mod agent_provenance_inventory;
mod agent_reads;
mod agent_title_claims;
mod agent_title_state;
mod agent_title_writes;
mod agent_titles;
mod agent_worker_capacity;
mod agent_worker_reconcile;
mod agent_worker_store;
mod agent_worker_transaction;
mod agents;
pub mod auth;
mod bindings;
pub(crate) mod bot_setups;
pub(crate) mod bots;
#[cfg(test)]
mod bots_mongo_integration_tests;
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
mod gateway_interactions;
mod gateway_records;
pub(crate) use gateway_interactions::MongoGatewayInteractionStore;
#[cfg(test)]
mod gateway_records_mongo_integration_tests;
mod hierarchy;
mod launch_context;
#[cfg(test)]
mod launch_context_mongo_integration_tests;
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
pub mod mongo;
pub mod operation;
mod operator_profiles;
mod operator_sandboxes;
mod profile_documents;
#[cfg(test)]
mod profile_index_tests;
#[cfg(test)]
mod profile_operator_mongo_integration_tests;
mod profile_references;
mod profiles;
mod resource_access;
mod sandbox_admission;
#[cfg(test)]
mod sandbox_admission_mongo_integration_tests;
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
mod sandbox_transactions;
mod sandbox_worker_store;
mod sandboxes;
#[cfg(test)]
mod sandboxes_tests;
mod schema;
#[cfg(test)]
mod schema_mongo_integration_tests;
pub mod settings;
mod shared_child_capacity;
mod trigger_definitions;
mod trigger_first_messages;
mod warm_sandboxes;
mod workspace_admission;
mod workspace_profiles;
mod workspace_removal;
mod workspace_worker_store;
mod workspaces;
mod worktrees;

pub use activity::MongoActivityStore;
pub use auth::MongoAuthStore;
pub(crate) use bindings::{MongoBindingCredentials, MongoProviderToolSource};
pub use checkpoint_objects::MongoCheckpointObjectStore;
pub(crate) use checkpoint_storage::{CheckpointBlobStore, ChronoStorage};
pub(crate) use codex_subscriptions::MongoSubscriptionStore;
pub(crate) use default_profiles::DefaultProfileSpec;
pub use directory::MongoUserDirectoryStore;
pub(crate) use encryption::AuthEncryptionKey;
pub(crate) use gateway_records::MongoGatewayRecordStore;
pub use mongo::MongoStores;
pub use operation::MongoOperationStore;
pub(crate) use sandbox_credentials::MongoSandboxCredentialStore;
pub use sandbox_mongo::{MongoProfileStore, MongoSandboxStore};
pub use settings::MongoPlatformSettingsStore;
pub use trigger_definitions::MongoTriggerDefinitionStore;
pub(crate) use warm_sandboxes::WarmSandboxClaim;

pub(crate) mod event_workers;
mod legacy_finalization;
mod legacy_reuse_lifecycle;
mod legacy_sandbox;
