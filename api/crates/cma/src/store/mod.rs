//! Durable adapters that do not depend on a resource aggregate.

pub mod activity;
pub mod auth;
pub mod directory;
mod durability;
mod encryption;
pub mod mongo;
pub mod operation;
pub mod settings;

pub use activity::MongoActivityStore;
pub use auth::MongoAuthStore;
pub use directory::MongoUserDirectoryStore;
pub(crate) use encryption::AuthEncryptionKey;
pub use mongo::MongoStores;
pub use operation::MongoOperationStore;
pub use settings::MongoPlatformSettingsStore;
