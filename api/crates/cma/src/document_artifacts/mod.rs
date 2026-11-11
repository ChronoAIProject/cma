//! Durable document metadata. Source objects are supplied by the independently
//! configured object repository; no document content is stored in this collection.
mod mongo;
mod quota;
mod quota_baseline;
pub(crate) mod quota_cli;
pub use mongo::MongoDocumentStore;
pub(crate) use mongo::COLLECTION;
pub(crate) use quota::{MongoDocumentQuotaStore, QUOTA_COLLECTION};

#[cfg(test)]
mod mongo_integration_tests;
