//! Durable document metadata. Source objects are supplied by the independently
//! configured object repository; no document content is stored in this collection.
mod commit;
mod mongo;
mod quota;
mod quota_baseline;
mod reclaim;
pub(crate) use reclaim::MongoDocumentReclaimStore;
pub(crate) mod quota_cli;
mod reconcile;
pub(crate) mod worker;
pub use mongo::MongoDocumentStore;
pub(crate) use mongo::COLLECTION;
pub(crate) use quota::{MongoDocumentQuotaStore, QUOTA_COLLECTION};

#[cfg(test)]
mod commit_mongo_tests;
#[cfg(test)]
mod mongo_integration_tests;

#[cfg(test)]
mod reclaim_mongo_tests;
