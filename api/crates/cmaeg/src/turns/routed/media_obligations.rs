//! Sealed per-attachment media ledger, independent of the original response
//! lease. No file bytes are stored here and no caller-selected path is authority.
#[path = "media_obligation_artifacts.rs"]
mod artifacts;
#[path = "media_obligation_text.rs"]
mod text;
use super::{model::Ownership, storage, ExecutionError, Journal};
use crate::{
    ingress::routing_store::{AdmittedPlan, StoredTarget},
    turns::render_media,
};
use cmaeg_core::media_continuation::{
    sha256, supplement_text, MediaArtifactClaim, MediaContinuationKind,
};
use serde::{Deserialize, Serialize};

const KIND: &str = "routed_media_obligation_v1";
const LEASE_SECONDS: i64 = 30;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MediaUploadReceipt {
    pub id: String,
    pub agent_id: String,
    pub name: String,
    pub path: String,
    pub size_bytes: u64,
    pub media_type: String,
    pub sha256: String,
}
impl MediaUploadReceipt {
    pub fn validate(&self, original: &MediaArtifactClaim) -> Result<(), ExecutionError> {
        if original.kind != MediaContinuationKind::Original
            || original.text_sha256.is_some()
            || self.name != original.name
            || self.size_bytes != original.size_bytes
            || self.media_type != original.media_type
            || self.sha256 != original.source_sha256
            || !safe_id(&self.agent_id)
            || !safe_id(&self.id)
            || self.path
                != format!(
                    "/home/attachments/{}/{}/{}",
                    self.agent_id, self.id, self.name
                )
        {
            return Err(ExecutionError::Invalid);
        }
        original.validate().map_err(|_| ExecutionError::Invalid)
    }
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MediaContinuationReceipt {
    pub command_id: String,
    pub response_id: String,
    pub metadata: std::collections::BTreeMap<String, String>,
}
impl MediaContinuationReceipt {
    fn valid(&self) -> bool {
        safe_id(&self.command_id)
            && safe_id(&self.response_id)
            && self.metadata.len() == 5
            && [
                "cma_parent_command_id",
                "cma_parent_response_id",
                "cma_parent_fingerprint",
                "cma_artifact_id",
                "cma_command_fingerprint",
            ]
            .iter()
            .all(|key| {
                self.metadata
                    .get(*key)
                    .is_some_and(|value| !value.is_empty() && value.len() <= 512)
            })
    }
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrozenMediaSupplement {
    pub artifact: MediaArtifactClaim,
    pub text: TextReference,
    pub admission: Option<MediaContinuationReceipt>,
    /// Submission alone never proves Agent execution or platform delivery.
    pub settled: bool,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextReference {
    pub sha256: String,
    pub len: usize,
    pub chunks: Vec<String>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MediaObligation {
    kind: String,
    pub(super) id: String,
    root: String,
    target: String,
    agent_id: String,
    pub(super) owner: String,
    request_hash: String,
    pub(super) retained_until: i64,
    pub(super) event_expires_at: i64,
    pub attachment_index: u8,
    pub gateway_binding_generation: Option<u64>,
    pub pending_derivative: Option<MediaContinuationKind>,
    pub original: Option<MediaArtifactClaim>,
    pub upload: Option<MediaUploadReceipt>,
    pub supplements: Vec<FrozenMediaSupplement>,
    pub refusal: Option<String>,
    pub(super) ownership: Option<Ownership>,
    #[serde(skip)]
    version: u64,
}
/// An opaque CAS ownership receipt. It never shares the original turn lease.
#[derive(Clone)]
pub struct MediaLease {
    pub(super) record: MediaObligation,
}
impl MediaLease {
    pub fn state(&self) -> &MediaObligation {
        &self.record
    }
}
#[derive(Clone)]
pub struct MediaObligationStore(pub Journal);
impl MediaObligationStore {
    pub async fn initialize(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
        index: u8,
        generation: Option<u64>,
        pending: Option<MediaContinuationKind>,
    ) -> Result<(), ExecutionError> {
        if !plan.targets.contains(target)
            || render_media::attachments(&plan.activity)
                .get(usize::from(index))
                .is_none()
            || generation == Some(0)
            || pending == Some(MediaContinuationKind::Original)
            || target.launch.provider.slug.as_str() != "cma_codex"
        {
            return Err(ExecutionError::Invalid);
        }
        let agent_id = selected_agent(target)?;
        if let Some(existing) = self.load(plan, target, index).await? {
            return if existing.gateway_binding_generation == generation
                && existing.pending_derivative == pending
            {
                Ok(())
            } else {
                Err(ExecutionError::IdempotencyConflict)
            };
        }
        let record = MediaObligation {
            kind: KIND.into(),
            id: identifier(target, index),
            root: plan.admission_id.clone(),
            target: target.id.clone(),
            agent_id: agent_id.into(),
            owner: plan.identity.owner.clone(),
            request_hash: plan.request_hash.clone(),
            retained_until: plan.retained_until,
            event_expires_at: target.launch.event.expires_at,
            attachment_index: index,
            gateway_binding_generation: generation,
            pending_derivative: pending,
            original: None,
            upload: None,
            supplements: Vec::new(),
            refusal: None,
            ownership: None,
            version: 0,
        };
        self.0
            .write(
                &record.id,
                &record.owner,
                record.retained_until,
                &record,
                None,
            )
            .await?;
        Ok(())
    }
    pub async fn load(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
        index: u8,
    ) -> Result<Option<MediaObligation>, ExecutionError> {
        let Some((mut record, version)) = self
            .0
            .read::<MediaObligation>(&identifier(target, index))
            .await?
        else {
            return Ok(None);
        };
        record.version = version;
        validate(&record, plan, target, index)?;
        Ok(Some(record))
    }
    pub async fn claim(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
        index: u8,
        worker: &str,
    ) -> Result<MediaLease, ExecutionError> {
        if worker.is_empty() || worker.len() > 128 {
            return Err(ExecutionError::Invalid);
        }
        let mut record = self
            .load(plan, target, index)
            .await?
            .ok_or(ExecutionError::Invalid)?;
        let now = (self.0.clock)();
        if record.retained_until <= now {
            return Err(ExecutionError::Expired);
        }
        if record
            .ownership
            .as_ref()
            .is_some_and(|owner| owner.until > now)
        {
            return Err(ExecutionError::Busy);
        }
        record.ownership = Some(Ownership {
            worker: worker.into(),
            claim: uuid::Uuid::new_v4().to_string(),
            epoch: record.ownership.as_ref().map_or(Ok(1), |owner| {
                owner.epoch.checked_add(1).ok_or(ExecutionError::Corrupt)
            })?,
            until: now + LEASE_SECONDS,
        });
        record.version = self
            .0
            .write(
                &record.id,
                &record.owner,
                record.retained_until,
                &record,
                Some(record.version),
            )
            .await?;
        Ok(MediaLease { record })
    }
    pub async fn guard(&self, lease: &MediaLease) -> Result<(), ExecutionError> {
        let record = &lease.record;
        let now = (self.0.clock)();
        if record.retained_until <= now {
            return Err(ExecutionError::Expired);
        }
        if record
            .ownership
            .as_ref()
            .is_none_or(|owner| owner.until <= now)
        {
            return Err(ExecutionError::LostLease);
        }
        let Some((stored, version)) = self.0.read::<MediaObligation>(&record.id).await? else {
            return Err(ExecutionError::LostLease);
        };
        if version != record.version
            || storage::bytes(&stored)? != storage::bytes(record)?
            || record
                .ownership
                .as_ref()
                .is_none_or(|owner| owner.until <= (self.0.clock)())
        {
            return Err(ExecutionError::LostLease);
        }
        Ok(())
    }
    /// Effect authority ends at the sealed event expiry; the ledger remains
    /// writable for terminal refusal/evidence until its longer retention ends.
    pub async fn guard_effect(&self, lease: &MediaLease) -> Result<(), ExecutionError> {
        self.guard(lease).await?;
        if lease.record.event_expires_at <= (self.0.clock)() {
            return Err(ExecutionError::Expired);
        }
        if lease.record.refusal.is_some() {
            return Err(ExecutionError::Invalid);
        }
        Ok(())
    }
    pub(super) async fn commit(
        &self,
        lease: &mut MediaLease,
        mut next: MediaObligation,
    ) -> Result<(), ExecutionError> {
        self.guard(lease).await?;
        next.version = self
            .0
            .write(
                &next.id,
                &next.owner,
                next.retained_until,
                &next,
                Some(lease.record.version),
            )
            .await?;
        lease.record = next;
        Ok(())
    }
    pub async fn renew(&self, lease: &mut MediaLease) -> Result<(), ExecutionError> {
        let mut next = lease.record.clone();
        next.ownership
            .as_mut()
            .ok_or(ExecutionError::LostLease)?
            .until = (self.0.clock)() + LEASE_SECONDS;
        self.commit(lease, next).await
    }
    pub async fn release(&self, lease: &mut MediaLease) -> Result<(), ExecutionError> {
        let mut next = lease.record.clone();
        next.ownership
            .as_mut()
            .ok_or(ExecutionError::LostLease)?
            .until = (self.0.clock)();
        self.commit(lease, next).await
    }
}

fn identifier(target: &StoredTarget, index: u8) -> String {
    storage::identifier(&(KIND, &target.id, index))
}
fn safe_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}
fn selected_agent(target: &StoredTarget) -> Result<&str, ExecutionError> {
    match &target.launch.provider.target {
        cmaeg_core::channel::ProviderTarget::ExistingAgent { agent_id } => Ok(agent_id),
        _ => Err(ExecutionError::Invalid),
    }
}
fn validate(
    record: &MediaObligation,
    plan: &AdmittedPlan,
    target: &StoredTarget,
    index: u8,
) -> Result<(), ExecutionError> {
    if record.kind != KIND
        || record.id != identifier(target, index)
        || record.root != plan.admission_id
        || record.target != target.id
        || selected_agent(target).ok() != Some(record.agent_id.as_str())
        || record.owner != plan.identity.owner
        || record.request_hash != plan.request_hash
        || record.retained_until != plan.retained_until
        || record.event_expires_at != target.launch.event.expires_at
        || record.attachment_index != index
        || record.gateway_binding_generation == Some(0)
        || record.pending_derivative == Some(MediaContinuationKind::Original)
        || record.supplements.len() > 2
        || render_media::attachments(&plan.activity)
            .get(usize::from(index))
            .is_none()
    {
        return Err(ExecutionError::Corrupt);
    }
    if let Some(original) = &record.original {
        if original.validate().is_err()
            || original.kind != MediaContinuationKind::Original
            || original.text_sha256.is_some()
            || original.attachment_index != index
        {
            return Err(ExecutionError::Corrupt);
        }
    }
    if let Some(upload) = &record.upload {
        if upload.agent_id != record.agent_id {
            return Err(ExecutionError::Corrupt);
        }
        upload
            .validate(record.original.as_ref().ok_or(ExecutionError::Corrupt)?)
            .map_err(|_| ExecutionError::Corrupt)?;
    }
    for (index, item) in record.supplements.iter().enumerate() {
        if item.artifact.validate().is_err()
            || item.artifact.text_sha256.is_none()
            || Some(&item.artifact.original()) != record.original.as_ref()
            || record.upload.is_none()
            || item
                .admission
                .as_ref()
                .is_some_and(|receipt| !receipt.valid())
            || item.settled && item.admission.is_none()
            || record.supplements[..index]
                .iter()
                .any(|other| other.artifact.kind == item.artifact.kind)
            || item.artifact.kind != MediaContinuationKind::Original
                && Some(item.artifact.kind) != record.pending_derivative
        {
            return Err(ExecutionError::Corrupt);
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "media_obligation_tests.rs"]
mod tests;
