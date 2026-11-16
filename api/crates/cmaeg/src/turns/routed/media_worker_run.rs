//! Execute one retained media head while preserving the captured FIFO owner.
use super::*;
use crate::replicas::partition::PartitionLease;

impl MediaWorker {
    pub(in crate::turns::routed) async fn run_owned(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
        partition: &mut Option<PartitionLease>,
    ) -> Result<bool, ExecutionError> {
        self.store.0.guard_partition(partition).await?;
        let Some(head) = self.index.load(plan, target).await? else {
            return if partition.is_some() {
                self.completed_media(plan, target).await
            } else {
                Ok(true)
            };
        };
        if partition.is_some() && self.completed_media(plan, target).await? {
            self.index.finish(&head).await?;
            return Ok(true);
        }
        let (parent, _) = self
            .store
            .0
            .read::<TargetState>(&target.id)
            .await?
            .ok_or(ExecutionError::Corrupt)?;
        validate_parent(&head, plan, target, &parent)?;
        let expired = head.event_expires_at <= (self.store.0.clock)();
        let refused = matches!(parent.phase, Phase::Refused { .. })
            || self.store.0.quarantined(plan, target).await?;
        if !expired
            && !refused
            && !matches!(
                parent.phase,
                Phase::Accepted { .. } | Phase::Completed { .. }
            )
        {
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
                    plan,
                    target,
                    index,
                    head.gateway_generation,
                    head.schedule.derivative(index),
                )
                .await?;
            let mut lease = self
                .store
                .claim_partition(plan, target, index, &self.worker, partition.clone())
                .await?;
            let result = self
                .process_attachment(plan, target, &mut lease, expired, refused)
                .await;
            // Even a failed later stage returns the same token's successfully
            // renewed deadline to the driver; no owner lookup is substituted.
            *partition = lease.partition.clone();
            let release = self.store.release(&mut lease).await;
            if let Err(error) = &release {
                tracing::warn!(
                    ?error,
                    "Media sublease release failed; retaining fenced custody"
                );
            }
            done &= result?;
            release?;
        }
        if done && !expired && !refused && !parent.phase.terminal() {
            return Ok(false);
        }
        if done {
            self.delivery
                .finish_partition(plan, target, partition)
                .await?;
            if let Some(token) = partition {
                self.complete_media(plan, target, token).await?;
            }
            self.index.finish(&head).await?;
        }
        Ok(done)
    }

    async fn process_attachment(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
        lease: &mut MediaLease,
        expired: bool,
        refused: bool,
    ) -> Result<bool, ExecutionError> {
        if self.unresolved_media(lease).await? {
            return Err(ExecutionError::Uncertain);
        }
        let result = if expired || refused {
            self.store
                .refuse(
                    lease,
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
            self.attachment(plan, target, lease).await
        };
        match result {
            Err(ExecutionError::AuthorityChanged | ExecutionError::Expired) => {
                // Expiry/revocation is not evidence that an already attempted
                // external send had no effect. Preserve its reconciliation hold.
                if self.unresolved_media(lease).await? {
                    return Err(ExecutionError::Uncertain);
                }
                self.store
                    .refuse(
                        lease,
                        if matches!(result, Err(ExecutionError::Expired)) {
                            "event_expired"
                        } else {
                            "authority_revoked"
                        },
                    )
                    .await?;
                Ok(true)
            }
            other => other,
        }
    }

    async fn unresolved_media(&self, lease: &MediaLease) -> Result<bool, ExecutionError> {
        if lease.partition.is_none() {
            return Ok(false);
        }
        for supplement in &lease.state().supplements {
            if supplement.admission.is_none() {
                continue;
            }
            if let Some(state) = self
                .store
                .retained_media_response(lease.state(), supplement.artifact.kind)
                .await?
            {
                if crate::turns::routed::media_delivery::summary(&state) == "unknown" {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }
}
