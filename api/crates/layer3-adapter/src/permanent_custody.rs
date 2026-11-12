//! Create-only GCS custody, independently verified with generation-pinned reads.
//! No update/delete/list-object API is exposed; admission rejects destructive IAM.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
pub type Result<T> = std::result::Result<T, &'static str>;
pub(super) const REFUSED: &str = "permanent archive custody could not be verified";
pub const MAX_OBJECT_BYTES: usize = 64 * 1024 * 1024;
mod gcs;
pub use gcs::GcsCustody;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CustodyPolicy {
    pub format: u32,
    pub bucket: String,
    pub metageneration: String,
    pub permanent_hold: bool,
    pub private_uniform_access: bool,
    pub independent_read_only: bool,
}
impl CustodyPolicy {
    pub fn sha256(&self) -> Result<String> {
        Ok(digest(&serde_json::to_vec(self).map_err(|_| REFUSED)?))
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObjectReceipt {
    pub key: String,
    pub generation: String,
    pub sha256: String,
    pub bytes: u64,
}
#[async_trait::async_trait]
pub trait PermanentCustody: Send + Sync {
    async fn policy(&self) -> Result<CustodyPolicy>;
    /// Create or reconcile the exact previously accepted content. Never replace.
    async fn retain(&self, operation: &str, bytes: Vec<u8>) -> Result<ObjectReceipt>;
    async fn read(&self, receipt: &ObjectReceipt) -> Result<Vec<u8>>;
}
pub fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub(super) fn key(operation: &str, bytes: &[u8]) -> Result<String> {
    let id = uuid::Uuid::parse_str(operation).map_err(|_| REFUSED)?;
    if id.is_nil() || id.to_string() != operation || bytes.len() > MAX_OBJECT_BYTES {
        return Err(REFUSED);
    }
    Ok(format!("retirement-v1/{operation}/{}", digest(bytes)))
}
pub(super) fn valid_key(key: &str) -> bool {
    let parts = key.split('/').collect::<Vec<_>>();
    parts.len() == 3
        && parts[0] == "retirement-v1"
        && uuid::Uuid::parse_str(parts[1])
            .is_ok_and(|id| !id.is_nil() && id.to_string() == parts[1])
        && parts[2].len() == 64
        && parts[2]
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

#[cfg(test)]
mod tests;
