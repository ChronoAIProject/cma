//! Immutable allocation policy and permanent reference identities. No expiry authority.
use layer2_core::bridge::allocation::{AllocationError, AllocationRecord};
use mongodb::bson::{Bson, Document};
use serde::{Deserialize, Serialize};

pub(super) const POLICY: &str = "channel_history_policy";
pub(super) const COUNT: &str = "channel_history_reference_count";
pub(super) const REFERENCES: &str = "channel_history_references";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ChannelHistoryDisposition {
    Retain,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ChannelHistoryPolicy {
    pub schema_version: u16,
    pub allocation_id: String,
    pub allocation_sha256: String,
    pub disposition: ChannelHistoryDisposition,
    pub pinned_at_ms: i64,
}
impl ChannelHistoryPolicy {
    pub fn capture(allocation: &AllocationRecord) -> Result<Self, AllocationError> {
        allocation.validate()?;
        Ok(Self {
            schema_version: 1,
            allocation_id: allocation.id.clone(),
            allocation_sha256: digest(&(
                "channel-history-allocation-v1",
                &allocation.identity,
                &allocation.owner,
                &allocation.pin,
                &allocation.config,
                &allocation.first_source,
                &allocation.placement,
                &allocation.operation_id,
                &allocation.predecessor,
                allocation.created_at_ms,
            ))?,
            disposition: ChannelHistoryDisposition::Retain,
            pinned_at_ms: allocation.created_at_ms,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ReferenceKind {
    FirstSource,
    AcceptedSource,
    AcceptedCommand,
    CallbackReceipt,
    OutboxItem,
    OutboxInvocation,
    LateUploadTombstone,
    PartitionCommand,
}
impl ReferenceKind {
    pub(super) fn collection(self) -> &'static str {
        match self {
            Self::FirstSource => "bridge_allocations",
            Self::AcceptedSource | Self::AcceptedCommand => "agent_commands",
            Self::CallbackReceipt => "bridge_callback_receipts",
            Self::OutboxItem => "bridge_outbox_items",
            Self::OutboxInvocation => "bridge_outbox_invocations",
            Self::LateUploadTombstone => "checkpoint_archive_tombstones",
            Self::PartitionCommand => "bridge_partition_command_receipts",
        }
    }
    pub(super) fn projection(self, document: &Document) -> Result<Bson, AllocationError> {
        let missing = || AllocationError::Corrupt;
        let value = match self {
            Self::FirstSource => document
                .get_document("record")
                .map_err(|_| missing())?
                .get("first_source")
                .cloned()
                .ok_or_else(missing)?,
            Self::AcceptedSource => document
                .get("channel_source")
                .cloned()
                .ok_or_else(missing)?,
            Self::AcceptedCommand => {
                let mut value = document
                    .get_document("channel_admission")
                    .map_err(|_| missing())?
                    .clone();
                value.remove("execution_hold");
                Bson::Document(value)
            }
            Self::OutboxItem => {
                let record = document.get_document("record").map_err(|_| missing())?;
                let mut value = Document::new();
                for field in [
                    "id",
                    "source",
                    "tool_invocation_id",
                    "turn_sequence",
                    "sequence",
                    "chunk_index",
                    "chunk_count",
                    "last_chunk",
                    "predecessor",
                    "content_sha256",
                    "text",
                    "created_at_ms",
                ] {
                    value.insert(field, record.get(field).cloned().ok_or_else(missing)?);
                }
                Bson::Document(value)
            }
            Self::CallbackReceipt | Self::OutboxInvocation | Self::PartitionCommand => {
                document.get("record").cloned().ok_or_else(missing)?
            }
            Self::LateUploadTombstone => {
                let mut value = Document::new();
                for field in ["_id", "document_schema_version", "object_key"] {
                    value.insert(field, document.get(field).cloned().ok_or_else(missing)?);
                }
                Bson::Document(value)
            }
        };
        Ok(value)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Reference {
    pub schema_version: u16,
    pub allocation_id: String,
    pub kind: ReferenceKind,
    pub target_id: String,
    pub target_sha256: String,
}
impl Reference {
    pub fn capture(
        allocation: &AllocationRecord,
        kind: ReferenceKind,
        target: &Document,
    ) -> Result<Self, AllocationError> {
        Ok(Self {
            schema_version: 1,
            allocation_id: allocation.id.clone(),
            kind,
            target_id: target
                .get_str("_id")
                .map_err(|_| AllocationError::Corrupt)?
                .into(),
            target_sha256: digest(&kind.projection(target)?)?,
        })
    }
    pub fn id(&self) -> Result<String, AllocationError> {
        Ok(format!(
            "chr_{}",
            digest(&(
                "channel-history-reference-v1",
                &self.allocation_id,
                self.kind,
                &self.target_id
            ))?
        ))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum HistoryHold {
    LegacyPolicyMissing,
    ReferenceAccountingMissing,
    ReferenceAccountingIncomplete,
    ReferencedRecordMissing,
    InventoryBoundExceeded,
    AcceptedWorkPending,
    OutboxUnresolved,
    ControlHistoryUnresolved,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct ChannelHistoryAssessment {
    pub policy: Option<ChannelHistoryPolicy>,
    pub reference_count: u64,
    pub reference_sha256: Option<String>,
    pub hold: Option<HistoryHold>,
    // Accepted work and late uploads may still add permanent references. A
    // matching current inventory cannot stand in for a sealed writer protocol.
    pub inventory_sealed: bool,
}

pub(super) fn digest(value: &impl Serialize) -> Result<String, AllocationError> {
    layer2_core::profiles::content_digest(value).map_err(|_| AllocationError::Corrupt)
}

pub(crate) fn policy(
    document: &Document,
    allocation: &AllocationRecord,
) -> Result<Option<ChannelHistoryPolicy>, AllocationError> {
    let Some(value) = document.get(POLICY) else {
        return Ok(None);
    };
    let policy: ChannelHistoryPolicy = mongodb::bson::deserialize_from_bson(value.clone())
        .map_err(|_| AllocationError::Corrupt)?;
    if policy != ChannelHistoryPolicy::capture(allocation)? {
        return Err(AllocationError::IdentityMismatch);
    }
    Ok(Some(policy))
}
