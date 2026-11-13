//! Reservations outlive uncertain object writes and metadata commits.
use super::{digest, valid_id, DocumentError, DocumentService, MAX_DOCUMENT_BYTES};
use serde::{Deserialize, Serialize};

pub const OWNER_DOCUMENT_LIMIT: u64 = 100;
pub const OWNER_DOCUMENT_BYTES: u64 = 1024 * 1024 * 1024;
// Failed writes with tiny bodies must not create an unbounded accounting ledger.
pub const OWNER_DOCUMENT_OBJECT_LIMIT: u64 = 32_768;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaEnforcement {
    Enforced,
    NotEnforced,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageCounter {
    pub used: u64,
    pub limit: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentUsage {
    pub enforcement: QuotaEnforcement,
    pub documents: Option<UsageCounter>,
    pub bytes: Option<UsageCounter>,
    pub objects: Option<UsageCounter>,
    pub pending_deletions: Option<u64>,
}

impl DocumentUsage {
    pub fn not_enforced() -> Self {
        Self {
            enforcement: QuotaEnforcement::NotEnforced,
            documents: None,
            bytes: None,
            objects: None,
            pending_deletions: None,
        }
    }
}

impl DocumentService {
    pub async fn usage(&self, caller: &crate::Caller) -> Result<DocumentUsage, DocumentError> {
        super::service::principal(caller)?;
        let mut usage = match &self.quota {
            Some(quota) => {
                quota
                    .usage(&digest(caller.subject.as_str().as_bytes()))
                    .await
            }
            None => Ok(DocumentUsage::not_enforced()),
        }?;
        if self.deletion_enabled {
            usage.pending_deletions = Some(
                self.store
                    .pending_deletions(&digest(caller.subject.as_str().as_bytes()))
                    .await?,
            );
        }
        Ok(usage)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentSourceReservation {
    pub owner_hash: String,
    pub document_id: String,
    pub sha256: String,
    pub bytes: u64,
}

impl DocumentSourceReservation {
    pub fn validate(&self) -> Result<(), DocumentError> {
        if !valid_digest(&self.owner_hash)
            || !valid_digest(&self.sha256)
            || !valid_id(&self.document_id, "doc_")
            || self.bytes == 0
            || self.bytes > MAX_DOCUMENT_BYTES as u64
        {
            return Err(DocumentError::Invalid);
        }
        Ok(())
    }

    pub fn object_key(&self) -> String {
        format!(
            "documents/{}/{}/{}.md",
            self.owner_hash, self.document_id, self.sha256
        )
    }

    pub(super) fn new(owner: &str, id: &str, bytes: &[u8]) -> Self {
        Self {
            owner_hash: digest(owner.as_bytes()),
            document_id: id.to_owned(),
            sha256: digest(bytes),
            bytes: bytes.len() as u64,
        }
    }
}

pub fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceTicket {
    Ledgered,
    Unledgered,
}

#[async_trait::async_trait]
pub trait DocumentQuotaStore: Send + Sync {
    /// Atomically reserve a document slot and immutable object bytes. Exact
    /// object retries are free; timeouts/cancellation never release reservations.
    async fn reserve(
        &self,
        source: &DocumentSourceReservation,
    ) -> Result<SourceTicket, DocumentError>;

    /// Report only authoritative owner counters while enforcement is ready.
    async fn usage(&self, owner_hash: &str) -> Result<DocumentUsage, DocumentError>;
}
