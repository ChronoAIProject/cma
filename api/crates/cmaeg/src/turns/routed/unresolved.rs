//! Owner-scoped reconciliation of one expired, uncertain original submission.
use super::{journal, ExecutionError, Journal, Phase, TargetState};
pub use crate::replicas::unresolved::{HeadPreview, ReleaseRequest, ReleaseResult};
use crate::{
    ingress::routing_store::{AdmittedPlan, PlanStore},
    replicas::partition::Partitions,
    replicas::unresolved::ReleaseRecord,
};
use cmaeg_core::ids::ChannelId;

#[derive(Clone)]
pub struct UnresolvedRelease {
    journal: Journal,
    plans: PlanStore,
    partitions: Partitions,
    enabled: bool,
}
impl UnresolvedRelease {
    pub(crate) fn new(
        journal: Journal,
        plans: PlanStore,
        partitions: Partitions,
        enabled: bool,
    ) -> Self {
        Self {
            journal,
            plans,
            partitions,
            enabled,
        }
    }

    async fn load(
        &self,
        channel: &ChannelId,
        owner: &str,
        partition: &str,
        operation: &str,
    ) -> Result<(AdmittedPlan, String, ReleaseRecord), ExecutionError> {
        let item = self.partitions.unresolved_item(partition, owner).await?;
        if item.identity.channel_id != *channel || item.identity.owner != owner {
            return Err(ExecutionError::AuthorityChanged);
        }
        let plan = self
            .plans
            .load(&item.identity)
            .await
            .map_err(super::dispatcher::admission_error)?
            .ok_or(ExecutionError::Corrupt)?;
        let target = plan
            .targets
            .iter()
            .find(|target| target.id == item.target)
            .ok_or(ExecutionError::Corrupt)?;
        if item.request_hash != plan.request_hash {
            return Err(ExecutionError::Corrupt);
        }
        let (value, version) = self
            .journal
            .read::<serde_json::Value>(&item.work_id)
            .await?
            .ok_or(ExecutionError::Expired)?;
        let record = if item.effect {
            super::tool_effects::abandonment_record(
                value,
                version,
                &plan,
                target,
                &item.work_id,
                operation,
            )?
        } else {
            let mut state: TargetState =
                serde_json::from_value(value).map_err(|_| ExecutionError::Corrupt)?;
            state.version = version;
            journal::validate(&state, &plan, target)?;
            let phase = match state.phase {
                Phase::Starting => "starting",
                Phase::Unknown => "unknown",
                _ => "not_unresolved",
            };
            let original_phase =
                serde_json::to_value(&state.phase).map_err(|_| ExecutionError::Corrupt)?;
            let unresolved =
                matches!(state.phase, Phase::Starting | Phase::Unknown) && state.delivery.is_none();
            state.phase = Phase::UnknownAbandoned {
                operation_id: operation.into(),
            };
            ReleaseRecord {
                id: item.work_id.clone(),
                version,
                retained_until: state.retained_until,
                expires_at: state.event_expires_at,
                phase,
                unresolved,
                effect: false,
                original_phase,
                abandoned: serde_json::to_value(state).map_err(|_| ExecutionError::Corrupt)?,
            }
        };
        Ok((plan, item.target, record))
    }

    pub async fn preview(
        &self,
        channel: &ChannelId,
        owner: &str,
        partition: &str,
    ) -> Result<HeadPreview, ExecutionError> {
        let (plan, id, state) = self.load(channel, owner, partition, "preview").await?;
        let target = plan
            .targets
            .iter()
            .find(|target| target.id == id)
            .ok_or(ExecutionError::Corrupt)?;
        let mut preview = self
            .partitions
            .preview_unknown(&plan, target, &state)
            .await?;
        preview.releasable &= self.enabled;
        Ok(preview)
    }

    #[cfg(test)]
    pub async fn release(
        &self,
        channel: &ChannelId,
        owner: &str,
        actor_token_hash: &str,
        request: &ReleaseRequest,
    ) -> Result<ReleaseResult, ExecutionError> {
        self.release_as(channel, owner, owner, actor_token_hash, request)
            .await
    }

    pub async fn release_as(
        &self,
        channel: &ChannelId,
        owner: &str,
        actor_subject: &str,
        actor_token_hash: &str,
        request: &ReleaseRequest,
    ) -> Result<ReleaseResult, ExecutionError> {
        if !self.enabled {
            return Err(ExecutionError::Unsupported);
        }
        if !request.provider_terminal_confirmed
            || !request.suppress_delayed_replies
            || request.expected_version == 0
            || actor_token_hash.is_empty()
            || actor_token_hash.len() > 128
            || actor_subject.is_empty()
            || actor_subject.len() > 512
            || [
                &request.partition_id,
                &request.expected_head,
                &request.operation_id,
            ]
            .into_iter()
            .any(|value| uuid::Uuid::parse_str(value).is_err())
        {
            return Err(ExecutionError::Invalid);
        }
        if let Some(result) = self
            .partitions
            .release_receipt(channel.as_str(), owner, actor_token_hash, request)
            .await?
        {
            return Ok(result);
        }
        let (plan, id, state) = self
            .load(channel, owner, &request.partition_id, &request.operation_id)
            .await?;
        let target = plan
            .targets
            .iter()
            .find(|target| target.id == id)
            .ok_or(ExecutionError::Corrupt)?;
        let result = self
            .partitions
            .abandon_unknown(
                &plan,
                target,
                &state,
                actor_token_hash,
                actor_subject,
                request,
            )
            .await?;
        tracing::warn!(
            partition_sha256 = super::diagnostic_id(&request.partition_id),
            target_sha256 = super::diagnostic_id(&result.released_head),
            operation_sha256 = super::diagnostic_id(&result.operation_id),
            disposition = "unknown_abandoned",
            epoch = result.epoch,
            "Authorized unresolved head release committed; original outcome remains uncertain"
        );
        Ok(result)
    }
}

pub(crate) fn release_enabled_from_env() -> Result<bool, &'static str> {
    match std::env::var("CMAEG_UNKNOWN_RELEASE_ENABLED") {
        Err(std::env::VarError::NotPresent) => Ok(false),
        Ok(value) if value == "false" => Ok(false),
        Ok(value) if value == "true" => Ok(true),
        _ => Err("CMAEG_UNKNOWN_RELEASE_ENABLED must be true or false"),
    }
}
