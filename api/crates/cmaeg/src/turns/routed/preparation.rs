//! Reader-first journal contract for explicit fresh-event binding preparation.
//! No production caller writes these checkpoints until the provider handshake
//! and ordinary-admission gate have been enabled together.
use super::{storage, ExecutionError, Journal, Lease, Phase};
use crate::ingress::routing_store::{AdmittedPlan, StoredTarget};
use serde::{Deserialize, Serialize};

/// Wire-equivalent to CMA's BindingPreparationIntent. These values contain no
/// source text or credentials; owner and manager are comparisons, not authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparationIntent {
    pub version: u8,
    pub admission_id: String,
    pub target_id: String,
    pub request_hash: String,
    pub binding_id: String,
    pub owner: String,
    pub manager: String,
    pub generation: u64,
    pub binding_revision: u64,
    pub binding_digest: String,
    pub sandbox_digest: String,
    pub workspace_digest: String,
    pub admitted_at: i64,
    pub expires_at: i64,
}
impl PreparationIntent {
    pub fn operation_id(&self) -> Result<String, ExecutionError> {
        Ok(format!(
            "bprep_{}",
            storage::hash(&storage::bytes(&(
                &self.admission_id,
                &self.target_id,
                &self.binding_id
            ))?)
        ))
    }
    pub fn fingerprint(&self) -> Result<String, ExecutionError> {
        Ok(storage::hash(&storage::bytes(self)?))
    }
    fn validate(&self, plan: &AdmittedPlan, target: &StoredTarget) -> Result<(), ExecutionError> {
        require_eligible(plan, target)?;
        if self.version != 1
            || self.admission_id != plan.admission_id
            || self.target_id != target.id
            || self.request_hash != plan.request_hash
            || self.binding_id != target.launch.provider.binding_id.as_str()
            || self.owner != plan.identity.owner
            || !bounded_id(&self.manager)
            || self.generation == 0
            || self.binding_revision == 0
            || self.admitted_at != plan.admitted_at
            || self.expires_at != target.launch.event.expires_at.min(plan.retained_until)
            || self.expires_at <= self.admitted_at
            || self.expires_at.saturating_sub(self.admitted_at) > 7 * 86400
            || [
                &self.request_hash,
                &self.binding_digest,
                &self.sandbox_digest,
                &self.workspace_digest,
            ]
            .into_iter()
            .any(|value| !valid_hash(value))
        {
            return Err(ExecutionError::Corrupt);
        }
        Ok(())
    }
}

/// Public projection of the provider's immutable private archive/receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparationReceipt {
    pub version: u8,
    pub operation_id: String,
    pub intent_fingerprint: String,
    pub generation: u64,
    pub sandbox_id: String,
    pub workspace_id: String,
}
impl PreparationReceipt {
    fn matches(&self, intent: &PreparationIntent) -> Result<(), ExecutionError> {
        if self.version != 1
            || self.operation_id != intent.operation_id()?
            || self.intent_fingerprint != intent.fingerprint()?
            || intent.generation.checked_add(1) != Some(self.generation)
            || !resource_id(&self.sandbox_id, "sbx_")
            || !resource_id(&self.workspace_id, "wks_")
        {
            return Err(ExecutionError::Corrupt);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum BindingPreparation {
    /// Healthy/provisioning hierarchy; no replacement request is necessary.
    Pinned {
        version: u8,
        binding_id: String,
        generation: u64,
    },
    /// Must be durably acknowledged before the first provider mutation.
    Pending { intent: PreparationIntent },
    /// Same immutable operation is replayed after restart or response loss.
    Reserved {
        intent: PreparationIntent,
        receipt: PreparationReceipt,
    },
}
impl BindingPreparation {
    pub(super) fn validate(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
    ) -> Result<(), ExecutionError> {
        require_eligible(plan, target)?;
        match self {
            Self::Pinned {
                version,
                binding_id,
                generation,
            } => {
                if *version != 1
                    || *generation == 0
                    || binding_id != target.launch.provider.binding_id.as_str()
                {
                    return Err(ExecutionError::Corrupt);
                }
                Ok(())
            }
            Self::Pending { intent } => intent.validate(plan, target),
            Self::Reserved { intent, receipt } => {
                intent.validate(plan, target)?;
                receipt.matches(intent)
            }
        }
    }
    pub(super) fn generation(&self) -> Option<u64> {
        match self {
            Self::Pinned { generation, .. } => Some(*generation),
            Self::Reserved { receipt, .. } => Some(receipt.generation),
            Self::Pending { .. } => None,
        }
    }
    pub(super) fn require_body_generation(&self, value: Option<u64>) -> Result<(), ExecutionError> {
        if self.generation().is_none() || self.generation() != value {
            return Err(ExecutionError::Corrupt);
        }
        Ok(())
    }
}

impl Journal {
    /// First write is immutable. A transport failure must reuse the retained
    /// intent; reinspection cannot silently choose a newer generation.
    pub async fn pin_binding_preparation(
        &self,
        lease: &mut Lease,
        plan: &AdmittedPlan,
        target: &StoredTarget,
        preparation: BindingPreparation,
    ) -> Result<(), ExecutionError> {
        self.guard(lease).await?;
        super::journal::validate(&lease.state, plan, target)?;
        preparation.validate(plan, target)?;
        if lease.state.event_expires_at <= (self.clock)() {
            return Err(ExecutionError::Expired);
        }
        if lease.state.phase != Phase::Pending
            || lease.state.body.is_some()
            || matches!(preparation, BindingPreparation::Reserved { .. })
        {
            return Err(ExecutionError::Invalid);
        }
        if let Some(existing) = &lease.state.binding_preparation {
            return if existing == &preparation {
                Ok(())
            } else {
                Err(ExecutionError::IdempotencyConflict)
            };
        }
        let mut next = lease.state.clone();
        next.binding_preparation = Some(preparation);
        self.commit(lease, next).await
    }

    pub async fn record_binding_preparation(
        &self,
        lease: &mut Lease,
        plan: &AdmittedPlan,
        target: &StoredTarget,
        receipt: PreparationReceipt,
    ) -> Result<(), ExecutionError> {
        self.guard(lease).await?;
        super::journal::validate(&lease.state, plan, target)?;
        if lease.state.phase != Phase::Pending || lease.state.body.is_some() {
            return Err(ExecutionError::Invalid);
        }
        let intent = match &lease.state.binding_preparation {
            Some(BindingPreparation::Pending { intent }) => intent.clone(),
            Some(BindingPreparation::Reserved {
                receipt: existing, ..
            }) if existing == &receipt => return Ok(()),
            _ => return Err(ExecutionError::IdempotencyConflict),
        };
        let preparation = BindingPreparation::Reserved { intent, receipt };
        preparation.validate(plan, target)?;
        let mut next = lease.state.clone();
        next.binding_preparation = Some(preparation);
        self.commit(lease, next).await
    }
}

fn require_eligible(plan: &AdmittedPlan, target: &StoredTarget) -> Result<(), ExecutionError> {
    if plan.binding_preparation_version != Some(1)
        || !plan.targets.contains(target)
        || !target.launch.provider.target.is_profile()
        || !plan.permits_binding_preparation()
    {
        return Err(ExecutionError::Invalid);
    }
    Ok(())
}
fn valid_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
fn bounded_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 512
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-:.".contains(&byte))
}
fn resource_id(value: &str, prefix: &str) -> bool {
    value
        .strip_prefix(prefix)
        .is_some_and(|suffix| !suffix.is_empty() && suffix.len() <= 128 && bounded_id(suffix))
}
