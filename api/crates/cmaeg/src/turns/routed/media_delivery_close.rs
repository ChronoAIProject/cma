//! Final stream closure follows durable supplementary delivery evidence.
use super::*;

impl MediaDelivery {
    pub(super) async fn finish_inner(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
        partition: &Option<crate::replicas::partition::PartitionLease>,
    ) -> Result<(), ExecutionError> {
        self.store.0.guard_partition(partition).await?;
        let mut failed = false;
        for (index, item) in crate::turns::render_media::attachments(&plan.activity)
            .iter()
            .enumerate()
        {
            if item.source.is_none() {
                continue;
            }
            let state = self
                .store
                .load(plan, target, index as u8)
                .await?
                .ok_or(ExecutionError::Corrupt)?;
            if state.refusal.is_some() {
                failed = true;
                continue;
            }
            let mut lease = self
                .store
                .claim_partition(plan, target, index as u8, "media-close", partition.clone())
                .await?;
            for supplement in &lease.state().supplements {
                let response = self
                    .store
                    .media_response(&lease, supplement.artifact.kind)
                    .await?
                    .ok_or(ExecutionError::Corrupt)?;
                failed |= response.status != "completed"
                    || matches!(summary(&response), "unknown" | "failed");
            }
            self.store.release(&mut lease).await?;
        }
        let id = super::super::storage::identifier(&("media_stream_close_v1", &target.id));
        if let Some((retained, _)) = self.store.0.read::<Value>(&id).await? {
            if retained["root"] != plan.admission_id || retained["target"] != target.id {
                return Err(ExecutionError::Corrupt);
            }
            if partition.is_some() && retained["outcome"] == "intent" {
                return Err(ExecutionError::Uncertain);
            }
            return Ok(());
        }
        let intent = json!({"kind":"media_stream_close_v1","root":plan.admission_id,"target":target.id,"failed":failed,"outcome":"intent"});
        self.store
            .0
            .write_fenced(
                partition,
                &id,
                &plan.identity.owner,
                plan.retained_until,
                &intent,
                None,
            )
            .await?;
        // The final negotiated frame restores the original terminal
        // payload. Revalidate AFTER the durable intent, even on failure.
        let authority = current_target(self.authority.as_ref(), plan, target)
            .await
            .and_then(|current| {
                if target.launch.event.expires_at <= (self.store.0.clock)() {
                    Err(ExecutionError::Expired)
                } else if !current
                    .definition
                    .reply
                    .sinks
                    .contains(&cmaeg_core::channel::Sink::Sync {})
                    || !plan.current_preparation_source(&current.definition, &self.store.0.keys)
                {
                    Err(ExecutionError::AuthorityChanged)
                } else {
                    Ok(())
                }
            });
        let refusal = authority.err().map(|error| match error {
            ExecutionError::Expired => Problem::new(
                ProblemCode::EventRefExpired,
                410,
                "media_stream_event_expired",
            ),
            ExecutionError::AuthorityChanged => Problem::new(
                ProblemCode::Forbidden,
                403,
                "media_stream_authority_revoked",
            ),
            _ => Problem::new(
                ProblemCode::ProviderUnavailable,
                503,
                "media_stream_authority_unavailable",
            ),
        });
        self.store.0.guard_partition(partition).await?;
        let local = self.coordinator.custom.as_ref().map(|custom| {
            let event = super::super::model::delivery_event(plan, target);
            match refusal.as_ref() {
                None => custom.finish_media(&event, failed),
                Some(refusal) => custom.refuse_media(&event, refusal.clone()),
            }
        });
        let outcome = json!({"kind":"media_stream_close_v1","root":plan.admission_id,"target":target.id,"failed":failed || refusal.is_some(),"outcome":local,"content_suppressed":refusal.is_some(),"refusal":refusal});
        self.store
            .0
            .write_fenced(
                partition,
                &id,
                &plan.identity.owner,
                plan.retained_until,
                &outcome,
                Some(1),
            )
            .await?;
        Ok(())
    }
}
