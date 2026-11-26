//! Durable document metadata. Source objects are supplied by the independently
//! configured object repository; no document content is stored in this collection.
pub(crate) mod deletion;
mod grants;
mod mongo;
mod object_authority;
mod object_reservations;
mod quota;
mod quota_baseline;
pub(crate) mod quota_cli;
mod reclamation_binding;
mod reclamation_slots;
mod reclamation_store;
mod reclamation_worker;
mod source_commit;
pub(crate) use mongo::decode as decode_record;
pub use mongo::MongoDocumentStore;
pub(crate) use mongo::COLLECTION;
pub(crate) use quota::{MongoDocumentQuotaStore, QUOTA_COLLECTION};
pub(crate) use reclamation_worker::ReclamationWorker;

#[cfg(test)]
mod mongo_integration_tests;
