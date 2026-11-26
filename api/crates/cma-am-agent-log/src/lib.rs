//! Agent log format and immutable record validation shared by the persistence adapters.

pub mod capacity;
mod codec;
pub mod digest;
mod envelope;
pub mod legacy;
pub mod publication;
pub mod writer_plan;

pub use codec::{Head, Seal};
pub use envelope::{read_format, require_agent_version, validate_identity};
use layer2_core::sandbox::store::SandboxStoreError;

pub const FORMAT: &str = "segmented_v1";
pub const READER: &str = "agent_log_segments_v1";
pub const MIGRATION: &str = "2026-11-05-am-log-segment-readers-v1";
pub const MAX_SEQUENCE: i64 = 9_007_199_254_740_991;
pub const MAX_ENTRIES: i64 = 10_128;
pub const MAX_BYTES: i64 = 65 * 1024 * 1024;
pub const MAX_SEAL_BYTES: usize = 1024 * 1024;

pub fn corrupt() -> SandboxStoreError {
    SandboxStoreError::Corrupt("Agent log segment identity, format or integrity is invalid".into())
}

pub fn required_nullable<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    <Option<T> as serde::Deserialize>::deserialize(deserializer)
}
