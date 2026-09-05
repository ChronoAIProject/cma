//! Durable adapters that do not depend on a resource aggregate.

pub mod activity;
mod agent_admission;
mod agent_admission_records;
mod agent_log;
mod agent_profiles;
mod agent_worker_store;
mod agents;
pub mod auth;
#[allow(dead_code)] // Layer 2 has not exposed the checkpoint port yet.
mod checkpoints;
mod default_profiles;
pub mod directory;
mod durability;
mod encryption;
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
mod profiles;
mod sandbox_admission;
mod sandbox_credentials;
mod sandbox_events;
mod sandbox_mongo;
#[cfg(test)]
mod sandbox_mongo_integration_tests;
#[cfg(test)]
mod sandbox_operator_mongo_integration_tests;
mod sandbox_profiles;
mod sandbox_transactions;
mod sandbox_worker_store;
mod sandboxes;
mod schema;
#[cfg(test)]
mod schema_mongo_integration_tests;
pub mod settings;
mod trigger_definitions;
mod workspace_admission;
mod workspace_profiles;
mod workspace_removal;
mod workspace_worker_store;
mod workspaces;
mod worktrees;

pub use activity::MongoActivityStore;
pub use auth::MongoAuthStore;
pub(crate) use default_profiles::DefaultProfileSpec;
pub use directory::MongoUserDirectoryStore;
pub(crate) use encryption::AuthEncryptionKey;
pub use mongo::MongoStores;
pub use operation::MongoOperationStore;
pub(crate) use sandbox_credentials::MongoSandboxCredentialStore;
pub use sandbox_mongo::{MongoProfileStore, MongoSandboxStore};
pub use settings::MongoPlatformSettingsStore;
pub use trigger_definitions::MongoTriggerDefinitionStore;
