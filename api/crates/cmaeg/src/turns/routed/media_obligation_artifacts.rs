//! Frozen artifact claims and admission receipts under an independent media lease.
use super::*;
impl MediaObligationStore {
    /// Freeze exact metadata/digest BEFORE the first original upload. An
    /// uncertain upload can retry only these bytes, never a refetched revision.
    pub async fn freeze_original(
        &self,
        lease: &mut MediaLease,
        artifact: MediaArtifactClaim,
    ) -> Result<(), ExecutionError> {
        artifact.validate().map_err(|_| ExecutionError::Invalid)?;
        if artifact.kind != MediaContinuationKind::Original
            || artifact.text_sha256.is_some()
            || artifact.attachment_index != lease.record.attachment_index
            || lease.record.refusal.is_some()
        {
            return Err(ExecutionError::Invalid);
        }
        if let Some(existing) = &lease.record.original {
            return if *existing == artifact {
                self.guard(lease).await
            } else {
                Err(ExecutionError::IdempotencyConflict)
            };
        }
        let mut next = lease.record.clone();
        next.original = Some(artifact);
        self.commit(lease, next).await
    }
    pub async fn record_upload(
        &self,
        lease: &mut MediaLease,
        receipt: MediaUploadReceipt,
    ) -> Result<(), ExecutionError> {
        if receipt.agent_id != lease.record.agent_id {
            return Err(ExecutionError::Invalid);
        }
        receipt.validate(
            lease
                .record
                .original
                .as_ref()
                .ok_or(ExecutionError::Invalid)?,
        )?;
        if let Some(existing) = &lease.record.upload {
            return if *existing == receipt {
                self.guard(lease).await
            } else {
                Err(ExecutionError::IdempotencyConflict)
            };
        }
        let mut next = lease.record.clone();
        next.upload = Some(receipt);
        self.commit(lease, next).await
    }
    pub async fn freeze_supplement(
        &self,
        lease: &mut MediaLease,
        mut artifact: MediaArtifactClaim,
        derived_text: &str,
    ) -> Result<MediaArtifactClaim, ExecutionError> {
        let original = lease
            .record
            .original
            .as_ref()
            .ok_or(ExecutionError::Invalid)?;
        let upload = lease
            .record
            .upload
            .as_ref()
            .ok_or(ExecutionError::Invalid)?;
        if &artifact.original() != original
            || lease.record.refusal.is_some()
            || artifact.kind != MediaContinuationKind::Original
                && Some(artifact.kind) != lease.record.pending_derivative
        {
            return Err(ExecutionError::Invalid);
        }
        let text = supplement_text(&artifact, &upload.path, derived_text)
            .map_err(|_| ExecutionError::Invalid)?;
        let digest = sha256(text.as_bytes());
        if artifact
            .text_sha256
            .as_ref()
            .is_some_and(|value| value != &digest)
        {
            return Err(ExecutionError::IdempotencyConflict);
        }
        artifact.text_sha256 = Some(digest);
        if let Some(existing) = lease
            .record
            .supplements
            .iter()
            .find(|item| item.artifact.kind == artifact.kind)
        {
            if existing.artifact != artifact
                || existing.text.sha256 != sha256(derived_text.as_bytes())
            {
                return Err(ExecutionError::IdempotencyConflict);
            }
            self.guard(lease).await?;
            return Ok(artifact);
        }
        let reference = self.write_text(lease, &artifact, derived_text).await?;
        let mut next = lease.record.clone();
        next.supplements.push(FrozenMediaSupplement {
            artifact: artifact.clone(),
            text: reference,
            admission: None,
            settled: false,
        });
        self.commit(lease, next).await?;
        Ok(artifact)
    }
    pub async fn record_submission(
        &self,
        lease: &mut MediaLease,
        kind: MediaContinuationKind,
        receipt: MediaContinuationReceipt,
    ) -> Result<(), ExecutionError> {
        if !receipt.valid()
            || receipt.metadata.get("cma_artifact_id")
                != Some(&format!(
                    "{}:{}",
                    lease.record.attachment_index,
                    kind.as_str()
                ))
        {
            return Err(ExecutionError::Invalid);
        }
        let (parent, _) = self
            .0
            .read::<crate::turns::routed::TargetState>(&lease.record.target)
            .await?
            .ok_or(ExecutionError::Corrupt)?;
        let parent_response = match &parent.phase {
            crate::turns::routed::Phase::Accepted { response_id, .. }
            | crate::turns::routed::Phase::Completed { response_id, .. } => response_id,
            _ => return Err(ExecutionError::Invalid),
        };
        if parent.root != lease.record.root
            || parent.request_hash != lease.record.request_hash
            || parent.owner != lease.record.owner
            || receipt.metadata.get("cma_parent_response_id") != Some(parent_response)
            || &receipt.response_id == parent_response
        {
            return Err(ExecutionError::Invalid);
        }
        let mut next = lease.record.clone();
        let supplement = next
            .supplements
            .iter_mut()
            .find(|item| item.artifact.kind == kind)
            .ok_or(ExecutionError::Invalid)?;
        if supplement
            .admission
            .as_ref()
            .is_some_and(|existing| existing != &receipt)
        {
            return Err(ExecutionError::IdempotencyConflict);
        }
        supplement.admission = Some(receipt);
        self.commit(lease, next).await
    }
    pub async fn refuse(&self, lease: &mut MediaLease, code: &str) -> Result<(), ExecutionError> {
        if code.is_empty()
            || code.len() > 128
            || !code.bytes().all(|v| v.is_ascii_lowercase() || v == b'_')
        {
            return Err(ExecutionError::Invalid);
        }
        let mut next = lease.record.clone();
        next.refusal = Some(code.into());
        self.commit(lease, next).await
    }
    /// This reads only server-created frozen claims. Event-only attestation is
    /// insufficient for uploading or enqueueing a new media continuation.
    pub async fn attests(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
        claim: &MediaArtifactClaim,
    ) -> Result<bool, ExecutionError> {
        if claim.validate().is_err() {
            return Ok(false);
        }
        let Some(record) = self.load(plan, target, claim.attachment_index).await? else {
            return Ok(false);
        };
        if record.refusal.is_some() || record.event_expires_at <= (self.0.clock)() {
            return Ok(false);
        }
        if claim.text_sha256.is_none() {
            return Ok(record.original.as_ref() == Some(claim));
        }
        Ok(record.upload.is_some()
            && record
                .supplements
                .iter()
                .any(|item| item.artifact == *claim))
    }
}
