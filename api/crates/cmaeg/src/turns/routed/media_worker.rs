//! Bounded continuation work is scheduled from active heads, never by scanning
//! every user's completed event history. It cannot submit an original event.
use super::{
    adapter::credential,
    builder::provider_error,
    dispatcher::{admission_error, current_target},
    media_work::{validate_parent, MediaWorkIndex},
    scheduler::{Disposition, TargetExecutor, WorkItem},
    Authority, ExecutionError, MediaLease, MediaObligationStore, Phase, TargetState,
};
use crate::{
    ingress::routing_store::{AdmittedPlan, PlanStore, StoredTarget},
    media::{
        derive::{Artifact, DeriveError, Derived},
        jobs::JobOutcome,
        service::MediaService,
        target::MediaTarget,
    },
    providers::ProviderClient,
    turns::render_media,
};
use cmaeg_core::media_continuation::{
    sha256, MediaArtifactClaim, MediaContinuationKind, MAX_CONTINUATION_TEXT_BYTES,
};
use futures_util::future::BoxFuture;
use std::{sync::Arc, time::Duration};

/// Response delivery is independently durable; accepting a supplement is never
/// considered completion. Its implementation must authenticate current scope.
pub(crate) trait MediaResponseDelivery: Send + Sync {
    fn deliver<'a>(
        &'a self,
        plan: &'a AdmittedPlan,
        target: &'a StoredTarget,
        lease: &'a mut MediaLease,
    ) -> BoxFuture<'a, Result<(), ExecutionError>>;
    fn finish<'a>(
        &'a self,
        plan: &'a AdmittedPlan,
        target: &'a StoredTarget,
    ) -> BoxFuture<'a, Result<(), ExecutionError>>;
}

pub(crate) struct MediaWorker {
    pub index: MediaWorkIndex,
    pub plans: PlanStore,
    pub store: MediaObligationStore,
    pub authority: Arc<dyn Authority>,
    pub providers: Arc<ProviderClient>,
    pub media: Arc<MediaService>,
    pub delivery: Arc<dyn MediaResponseDelivery>,
    pub worker: String,
}
impl MediaWorker {
    async fn run(&self, work: &WorkItem) -> Result<bool, ExecutionError> {
        let plan = self
            .plans
            .load(&work.identity)
            .await
            .map_err(admission_error)?
            .ok_or(ExecutionError::Corrupt)?;
        let target = plan
            .targets
            .iter()
            .find(|t| t.id == work.target_id)
            .ok_or(ExecutionError::Corrupt)?;
        let Some(head) = self.index.load(&plan, target).await? else {
            return Ok(true);
        };
        let (parent, _) = self
            .store
            .0
            .read::<TargetState>(&target.id)
            .await?
            .ok_or(ExecutionError::Corrupt)?;
        validate_parent(&head, &plan, target, &parent)?;
        let expired = head.event_expires_at <= (self.store.0.clock)();
        let refused = matches!(parent.phase, Phase::Refused { .. });
        if !expired
            && !refused
            && !matches!(
                parent.phase,
                Phase::Accepted { .. } | Phase::Completed { .. }
            )
        {
            // Unknown is not proof of acceptance. Do not upload, append, or
            // replay the original while its acceptance cannot be established.
            return Ok(false);
        }
        let mut done = true;
        for (index, attachment) in render_media::attachments(&plan.activity).iter().enumerate() {
            if attachment.source.is_none() {
                continue;
            }
            let index = u8::try_from(index).map_err(|_| ExecutionError::Corrupt)?;
            self.store
                .initialize(
                    &plan,
                    target,
                    index,
                    head.gateway_generation,
                    head.schedule.derivative(index),
                )
                .await?;
            let mut lease = self.store.claim(&plan, target, index, &self.worker).await?;
            let result = if expired || refused {
                self.store
                    .refuse(
                        &mut lease,
                        if expired {
                            "event_expired"
                        } else {
                            "parent_refused"
                        },
                    )
                    .await
                    .map(|_| true)
            } else if lease.state().refusal.is_some() {
                Ok(true)
            } else {
                self.attachment(&plan, target, &mut lease).await
            };
            let result = match result {
                Err(ExecutionError::AuthorityChanged) => {
                    self.store.refuse(&mut lease, "authority_revoked").await?;
                    Ok(true)
                }
                Err(ExecutionError::Expired) => {
                    self.store.refuse(&mut lease, "event_expired").await?;
                    Ok(true)
                }
                other => other,
            };
            let release = self.store.release(&mut lease).await;
            done &= result?;
            release?;
        }
        if done && !expired && !parent.phase.terminal() {
            return Ok(false);
        }
        if done {
            self.delivery.finish(&plan, target).await?;
            self.index.finish(&head).await?;
        }
        Ok(done)
    }
    async fn current(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
        lease: &mut MediaLease,
    ) -> Result<Arc<crate::channels::resolver::ResolvedChannel>, ExecutionError> {
        self.store.renew(lease).await?;
        self.store.guard_effect(lease).await?;
        let current = current_target(self.authority.as_ref(), plan, target).await?;
        if !plan.current_preparation_source(&current.definition, &self.store.0.keys) {
            return Err(ExecutionError::AuthorityChanged);
        }
        self.store.guard_effect(lease).await?;
        Ok(current)
    }
    async fn attachment(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
        lease: &mut MediaLease,
    ) -> Result<bool, ExecutionError> {
        self.upload(plan, target, lease).await?;
        // The original-file append is always independent of any late transcript.
        if !lease
            .state()
            .supplements
            .iter()
            .any(|s| s.artifact.kind == MediaContinuationKind::Original)
        {
            let original = lease
                .state()
                .original
                .clone()
                .ok_or(ExecutionError::Corrupt)?;
            self.store.freeze_supplement(lease, original, "").await?;
        }
        self.submit(plan, target, lease, MediaContinuationKind::Original)
            .await?;
        // A transient transcript failure must not delay an already completed
        // original-file response or hide it from the channel's recipient.
        self.delivery.deliver(plan, target, lease).await?;
        let pending = lease.state().pending_derivative;
        if let Some(kind) = pending {
            if !lease
                .state()
                .supplements
                .iter()
                .any(|s| s.artifact.kind == kind)
            {
                let current = self.current(plan, target, lease).await?;
                let media_target = MediaTarget::new(
                    current,
                    super::model::delivery_event(plan, target),
                    Arc::new(plan.activity.clone()),
                )
                .map_err(|_| ExecutionError::Corrupt)?;
                let outcome = self
                    .media
                    .derive(
                        &media_target,
                        usize::from(lease.state().attachment_index),
                        Artifact::Transcript,
                        Duration::from_secs(2),
                    )
                    .await;
                match outcome {
                    JobOutcome::Ready(value) => {
                        let Derived::Transcript {
                            document,
                            source_sha256,
                            ..
                        } = value.as_ref()
                        else {
                            return Err(ExecutionError::Corrupt);
                        };
                        let mut artifact = lease
                            .state()
                            .original
                            .clone()
                            .ok_or(ExecutionError::Corrupt)?;
                        if &artifact.source_sha256 != source_sha256 {
                            return Err(ExecutionError::IdempotencyConflict);
                        }
                        let text = document.pages.concat();
                        if text.len() > MAX_CONTINUATION_TEXT_BYTES {
                            return Err(ExecutionError::Invalid);
                        }
                        artifact.kind = kind;
                        artifact.content_sha256 = sha256(text.as_bytes());
                        self.current(plan, target, lease).await?;
                        self.store.freeze_supplement(lease, artifact, &text).await?;
                    }
                    JobOutcome::Pending { .. } => {}
                    JobOutcome::Failed(error) => return Err(derive_error(error)),
                }
            }
            if lease
                .state()
                .supplements
                .iter()
                .any(|s| s.artifact.kind == kind)
            {
                self.submit(plan, target, lease, kind).await?;
            }
        }
        if pending.is_some() {
            self.delivery.deliver(plan, target, lease).await?;
        }
        Ok(lease.state().supplements.iter().all(|s| s.settled)
            && pending.is_none_or(|kind| {
                lease
                    .state()
                    .supplements
                    .iter()
                    .any(|s| s.artifact.kind == kind)
            }))
    }
    async fn upload(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
        lease: &mut MediaLease,
    ) -> Result<(), ExecutionError> {
        if lease.state().upload.is_some() {
            return Ok(());
        }
        if let Some(original) = lease.state().original.clone() {
            // First recover an acknowledged durable receipt. This does not
            // require the native platform to still serve its original bytes.
            let current = self.current(plan, target, lease).await?;
            if let Some(receipt) = self
                .providers
                .event_media_upload_receipt(target, &original, credential(&current, target)?)
                .await
                .map_err(provider_error)?
            {
                self.store.guard_effect(lease).await?;
                return self.store.record_upload(lease, receipt).await;
            }
        }
        let current = self.current(plan, target, lease).await?;
        let media_target = MediaTarget::new(
            current,
            super::model::delivery_event(plan, target),
            Arc::new(plan.activity.clone()),
        )
        .map_err(|_| ExecutionError::Corrupt)?;
        let index = lease.state().attachment_index;
        let original = self
            .media
            .original_for_continuation(
                &media_target,
                usize::from(index),
                tokio::time::Instant::now() + Duration::from_secs(20),
            )
            .await
            .map_err(derive_error)?;
        let attachment = media_target
            .attachment(usize::from(index))
            .ok_or(ExecutionError::Corrupt)?;
        let digest = sha256(&original.bytes);
        let frozen = MediaArtifactClaim {
            attachment_index: index,
            kind: MediaContinuationKind::Original,
            content_sha256: digest.clone(),
            source_sha256: digest,
            name: safe_filename(attachment.filename.as_deref(), index),
            media_type: crate::media::sniff::sniff(
                &original.bytes[..original.bytes.len().min(4096)],
            )
            .mime
            .into(),
            size_bytes: original.bytes.len() as u64,
            text_sha256: None,
        };
        self.store.freeze_original(lease, frozen.clone()).await?;
        let current = self.current(plan, target, lease).await?;
        let receipt = self
            .providers
            .upload_event_media(
                target,
                lease.state().gateway_binding_generation,
                &frozen,
                original.bytes.clone(),
                credential(&current, target)?,
            )
            .await
            .map_err(provider_error)?;
        self.store.guard_effect(lease).await?;
        self.store.record_upload(lease, receipt).await
    }
    async fn submit(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
        lease: &mut MediaLease,
        kind: MediaContinuationKind,
    ) -> Result<(), ExecutionError> {
        let item = lease
            .state()
            .supplements
            .iter()
            .find(|s| s.artifact.kind == kind)
            .ok_or(ExecutionError::Corrupt)?;
        if item.admission.is_some() {
            return Ok(());
        }
        let artifact = item.artifact.clone();
        let text = self.store.derived_text(lease, &artifact).await?;
        let current = self.current(plan, target, lease).await?;
        let receipt = self
            .providers
            .supplement_event_media(
                target,
                lease.state().gateway_binding_generation,
                &artifact,
                &text,
                credential(&current, target)?,
            )
            .await
            .map_err(provider_error)?;
        self.store.guard_effect(lease).await?;
        self.store.record_submission(lease, kind, receipt).await
    }
}
impl TargetExecutor for MediaWorker {
    fn execute<'a>(&'a self, work: &'a WorkItem) -> BoxFuture<'a, Disposition> {
        Box::pin(async move {
            match self.run(work).await {
                Ok(true) => Disposition::Settled,
                _ => Disposition::Retry(Duration::from_secs(3)),
            }
        })
    }
}
fn derive_error(error: DeriveError) -> ExecutionError {
    match error {
        DeriveError::RateLimited => ExecutionError::Busy,
        _ => ExecutionError::Unavailable,
    }
}
fn safe_filename(name: Option<&str>, index: u8) -> String {
    let value: String = name
        .unwrap_or("")
        .chars()
        .take(120)
        .map(|c| {
            if c.is_ascii_alphanumeric() || "._-".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    if value.is_empty() || value.starts_with('.') {
        format!("attachment-{index}")
    } else {
        value
    }
}
#[cfg(test)]
#[path = "media_worker_tests.rs"]
mod tests;
