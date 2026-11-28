//! Durable MongoDB representations of Trigger definitions and Launches.

use layer2_core::trigger_domain::LaunchStoreError;

pub mod definitions;
pub mod launches;
pub mod validation;

const DOCUMENT_SCHEMA_VERSION: i64 = 1;

pub fn corrupt(detail: impl Into<String>) -> LaunchStoreError {
    LaunchStoreError::Corrupt(detail.into())
}
