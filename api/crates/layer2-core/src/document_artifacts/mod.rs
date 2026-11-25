//! Personal durable documents, independent of the environment that produced them.
//! Metadata and access change atomically; immutable source bytes live behind a
//! separate bounded object repository. No caller selects an object location.
mod blob_maintenance;
mod deletion;
mod mutation;
mod quota;
mod recipients;
mod runtime;
mod service;
mod source_authority;
mod standing;
mod types;
mod validation;
pub use blob_maintenance::{
    valid_document_blob_prefix, BlobDeletion, BlobMaintenanceError, BlobPage,
    DocumentBlobMaintenance, MAX_BLOB_CURSOR_BYTES, MAX_BLOB_PAGE_KEYS,
};
pub use deletion::{DocumentDeletionReceipt, DocumentTombstone};
pub use quota::*;
pub use service::DocumentService;
pub use source_authority::{DocumentSourceFence, DocumentSourceWrite, MAX_SOURCE_GENERATION};
pub use types::*;
#[cfg(any(test, feature = "test-support"))]
pub mod memory;
#[cfg(test)]
mod tests;

#[cfg(test)]
mod runtime_tests;

#[cfg(test)]
mod revocation_tests;

#[cfg(test)]
mod revocation_race_tests;

#[cfg(test)]
mod rollout_tests;

#[cfg(test)]
mod quota_tests;

#[cfg(test)]
mod blob_maintenance_tests;
#[cfg(test)]
mod recipient_tests;
#[cfg(test)]
mod standing_tests;
