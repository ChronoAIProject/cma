//! Durable adapters that do not depend on a resource aggregate.

pub mod activity;
mod agent_log;
mod agent_profiles;
mod agents;
pub mod auth;
#[allow(dead_code)] // Layer 2 has not exposed the checkpoint port yet.
mod checkpoints;
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
mod profile_documents;
#[cfg(test)]
mod profile_index_tests;
mod profiles;
mod sandbox_events;
mod sandbox_mongo;
#[cfg(test)]
mod sandbox_mongo_integration_tests;
mod sandbox_profiles;
mod sandbox_transactions;
mod sandboxes;
mod schema;
#[cfg(test)]
mod schema_mongo_integration_tests;
pub mod settings;
mod workspace_profiles;
mod workspaces;
mod worktrees;

pub use activity::MongoActivityStore;
pub use auth::MongoAuthStore;
pub use directory::MongoUserDirectoryStore;
pub(crate) use encryption::AuthEncryptionKey;
pub use mongo::MongoStores;
pub use operation::MongoOperationStore;
pub use sandbox_mongo::{MongoProfileStore, MongoSandboxStore};
pub use settings::MongoPlatformSettingsStore;
