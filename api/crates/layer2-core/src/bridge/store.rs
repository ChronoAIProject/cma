use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::resource_access::{ResourceActor, ResourceOwner};

use super::{
    AcceptedWorkPolicy, BridgeConfig, BridgeCustodyPin, BridgeEmergencyControls, BridgeError,
    BridgeId, BridgeRecord,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum BridgeChange {
    Create {
        owner: ResourceOwner,
        config: Box<BridgeConfig>,
    },
    Update {
        config: Box<BridgeConfig>,
        previous_work: Option<AcceptedWorkPolicy>,
    },
    Enable,
    Disable {
        accepted_work: AcceptedWorkPolicy,
    },
    Retire {
        accepted_work: AcceptedWorkPolicy,
    },
    Emergency {
        controls: BridgeEmergencyControls,
    },
}

impl BridgeChange {
    pub(super) fn name(&self) -> &'static str {
        match self {
            Self::Create { .. } => "create",
            Self::Update { .. } => "update",
            Self::Enable => "enable",
            Self::Disable { .. } => "disable",
            Self::Retire { .. } => "retire",
            Self::Emergency { .. } => "emergency",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeReceipt {
    pub bridge_id: BridgeId,
    pub owner: ResourceOwner,
    pub actor: ResourceActor,
    pub revision: u64,
    pub config_revision: u64,
    pub command: String,
    pub applied_at: String,
}

/// Internal idempotency envelope. No raw key or connector credential is stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredBridgeReceipt {
    pub id: String,
    pub command_digest: String,
    pub receipt: BridgeReceipt,
}

/// Constructed only by the authorized service. Stores must recheck the deadline
/// after obtaining their mutation lock/transaction and before committing.
#[derive(Debug, Clone)]
pub struct BridgeWrite {
    pub(super) expected_revision: u64,
    pub(super) record: BridgeRecord,
    pub(super) receipt: StoredBridgeReceipt,
    pub(super) deadline: tokio::time::Instant,
    pub(super) custody: Option<BridgeCustodyPin>,
    pub(super) enable_grant: Option<super::invoke::BridgeInvokeEnableProof>,
}

impl BridgeWrite {
    pub fn expected_revision(&self) -> u64 {
        self.expected_revision
    }
    pub fn record(&self) -> &BridgeRecord {
        &self.record
    }
    pub fn receipt(&self) -> &StoredBridgeReceipt {
        &self.receipt
    }
    /// Present when this transition requires a verified live connection. The
    /// durable store must recheck its exact active custody before writing.
    pub fn custody_pin(&self) -> Option<&BridgeCustodyPin> {
        self.custody.as_ref()
    }

    pub fn enable_grant(&self) -> Option<&super::invoke::BridgeInvokeEnableProof> {
        self.enable_grant.as_ref()
    }

    pub fn require_current(&self) -> Result<(), BridgeError> {
        if tokio::time::Instant::now() >= self.deadline {
            return Err(BridgeError::AuthorityUnavailable);
        }
        self.record.validate()?;
        if self.receipt.receipt.command == "enable"
            && matches!(
                self.record.current()?.config.placement,
                super::BridgePlacementPolicy::ExistingAgent { .. }
            )
        {
            self.enable_grant
                .as_ref()
                .ok_or(BridgeError::ConnectionRequired(
                    super::BridgeSetupBlock::InvokeConsentRequired,
                ))?
                .require_write(
                    &self.record,
                    &self.receipt.receipt.actor,
                    self.expected_revision,
                )?;
        } else if self.enable_grant.is_some() {
            return Err(BridgeError::Corrupt);
        }
        if self.custody.as_ref().is_some_and(|pin| {
            self.record
                .current()
                .is_ok_and(|generation| pin.reference() != &generation.config.connector.custody)
        }) {
            return Err(BridgeError::Corrupt);
        }
        if self.record.revision
            != self
                .expected_revision
                .checked_add(1)
                .ok_or(BridgeError::Conflict)?
            || self.receipt.receipt.bridge_id != self.record.id
            || self.receipt.receipt.owner != self.record.owner
            || self.receipt.receipt.revision != self.record.revision
            || self.receipt.receipt.config_revision != self.record.config_revision
        {
            return Err(BridgeError::Corrupt);
        }
        Ok(())
    }
}

#[async_trait::async_trait]
pub trait BridgeStore: Send + Sync {
    async fn bridge(&self, id: &BridgeId) -> Result<Option<BridgeRecord>, BridgeError>;
    async fn receipt(&self, id: &str) -> Result<Option<StoredBridgeReceipt>, BridgeError>;
    /// Order by ID, filter the exact owner before paging, and return at most
    /// limit+1 rows. The service authorizes this owner before invoking the port.
    async fn list(
        &self,
        owner: &ResourceOwner,
        issuer: &str,
        after: Option<&BridgeId>,
        limit: u32,
    ) -> Result<Vec<BridgeRecord>, BridgeError>;
    /// Atomically persist aggregate and receipt. Same-key same-body replay wins
    /// before revision checks; a reused key with different body always conflicts.
    /// When custody_pin is present, durable adapters must check that exact
    /// active custody in the same transaction and conflict with revocation.
    /// When enable_grant is present, compare/fence the exact current active
    /// grant and retained resource identity in that same transaction. Adapters
    /// without this shared boundary must refuse the write.
    async fn apply(&self, write: BridgeWrite) -> Result<StoredBridgeReceipt, BridgeError>;
}

pub(super) fn digest(value: &impl Serialize) -> Result<String, BridgeError> {
    let bytes = serde_json::to_vec(value).map_err(|_| BridgeError::Corrupt)?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

pub fn match_bridge_replay(
    stored: &StoredBridgeReceipt,
    requested: &StoredBridgeReceipt,
) -> Result<StoredBridgeReceipt, BridgeError> {
    if stored.id != requested.id
        || stored.command_digest != requested.command_digest
        || stored.receipt.bridge_id != requested.receipt.bridge_id
        || stored.receipt.owner != requested.receipt.owner
        || stored.receipt.actor != requested.receipt.actor
    {
        return Err(BridgeError::IdempotencyConflict);
    }
    Ok(stored.clone())
}
