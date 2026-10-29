//! Manager-only inspection and guarded replacement of a retired binding home.
use super::ProviderBindings;
use crate::{IdempotencyKey, Result};
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};

/// Immutable admission returned by preparation inspection; retain unchanged for replay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BindingPreparationIntent {
    /// Protocol version, currently 1.
    pub version: u8,
    /// Durable event admission identity.
    pub admission_id: String,
    /// Admitted consumer identity.
    pub target_id: String,
    /// Canonical admitted request digest.
    pub request_hash: String,
    /// Logical binding identity.
    pub binding_id: String,
    /// Resource owner subject.
    pub owner: String,
    /// Authenticated managing service subject.
    pub manager: String,
    /// Original generation.
    pub generation: u64,
    /// Original binding revision.
    pub binding_revision: u64,
    /// Original binding snapshot digest.
    pub binding_digest: String,
    /// Original Sandbox snapshot digest.
    pub sandbox_digest: String,
    /// Original Workspace snapshot digest.
    pub workspace_digest: String,
    /// Server admission Unix timestamp in seconds.
    pub admitted_at: i64,
    /// Admission expiration Unix timestamp in seconds.
    pub expires_at: i64,
}

/// Inspection input. Debug formatting omits the scoped credential.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InspectBindingPreparation {
    /// Request schema version, currently 1.
    pub schema_version: u16,
    /// Durable event admission identity.
    pub admission_id: String,
    /// Admitted consumer identity.
    pub target_id: String,
    /// Canonical admitted request digest.
    pub request_hash: String,
    /// Resource owner subject.
    pub owner: String,
    /// Server admission Unix timestamp in seconds.
    pub admitted_at: i64,
    /// Admission expiration Unix timestamp in seconds.
    pub expires_at: i64,
    /// Current binding credential, distinct from management authentication.
    pub agent_key: String,
}
impl std::fmt::Debug for InspectBindingPreparation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InspectBindingPreparation")
            .field("schema_version", &self.schema_version)
            .field("admission_id", &self.admission_id)
            .finish_non_exhaustive()
    }
}

/// Exact immutable intent and current credential used to reserve a replacement.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrepareBinding {
    /// Request schema version, currently 1.
    pub schema_version: u16,
    /// Exact intent returned by inspection.
    pub intent: BindingPreparationIntent,
    /// Current binding credential, omitted from Debug output.
    pub agent_key: String,
}
impl std::fmt::Debug for PrepareBinding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PrepareBinding")
            .field("schema_version", &self.schema_version)
            .field("intent", &self.intent)
            .finish_non_exhaustive()
    }
}

/// Read-only inspection result; a pending intent has not yet reserved anything.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum BindingPreparationInspection {
    /// The existing generation remains the target.
    Pinned {
        /// Protocol version.
        version: u8,
        /// Logical binding identity.
        binding_id: String,
        /// Current generation.
        generation: u64,
    },
    /// The original home is terminal and eligible for guarded reservation.
    Pending {
        /// Immutable inspection intent.
        intent: Box<BindingPreparationIntent>,
    },
}

/// Acknowledged immutable reservation; exact replay returns this same receipt.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct BindingPreparationReceipt {
    /// Protocol version.
    pub version: u8,
    /// Exact retained idempotency operation identity.
    pub operation_id: String,
    /// Canonical intent digest.
    pub intent_fingerprint: String,
    /// Reserved generation.
    pub generation: u64,
    /// Reserved Sandbox identity.
    pub sandbox_id: String,
    /// Reserved Workspace identity.
    pub workspace_id: String,
}

impl ProviderBindings<'_> {
    /// Inspect without creating, mutating, recovering, or rotating resources.
    pub async fn inspect_preparation(
        &self,
        id: &str,
        body: &InspectBindingPreparation,
    ) -> Result<BindingPreparationInspection> {
        self.0
            .operation(
                Method::POST,
                &["cmaeg", "bindings", id, "preparation-inspections"],
                Some(body),
                StatusCode::OK,
            )
            .await
    }

    /// Reserve with the retained intent and exact operation key; no automatic retry.
    pub async fn prepare(
        &self,
        id: &str,
        body: &PrepareBinding,
        key: &IdempotencyKey,
    ) -> Result<BindingPreparationReceipt> {
        self.0
            .command(
                Method::POST,
                &["cmaeg", "bindings", id, "preparations"],
                Some(body),
                key,
                StatusCode::OK,
            )
            .await
    }
}
