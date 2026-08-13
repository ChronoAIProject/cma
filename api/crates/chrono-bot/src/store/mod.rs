//! Durable storage, wired to concrete backends.
//!
//! The core defines the ports; these are the implementations. Keeping them in
//! the binary is what lets the core stay free of any particular database or
//! object store.

pub mod context;
pub mod mongo;

pub use context::ChronoStorageContextStore;
pub use mongo::MongoSessionStore;
