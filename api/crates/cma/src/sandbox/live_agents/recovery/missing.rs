use super::*;
use layer2_core::sandbox::store::AgentConnectionRecoveryCandidate;
use layer2_core::sandbox::AgentWorkStep;

impl LiveAgentRegistry {
    pub(in crate::sandbox::live_agents) async fn recover_unresolved(
        &self,
        target: &AgentRuntimeTarget,
    ) {
        // Only a live execution owner may request a new connection publication.
        // The store also fences this exact owner in its revision CAS.
        if !matches!(self.attachment_owner(&target.agent_id), Ok(Some(_))) {
            tracing::info!(agent_id = %target.agent_id, reason = "owner_mismatch",
                "Claimed command attachment recovery deferred");
            return;
        }
        if let Some(handler) = self.loss_handler.get() {
            handler.missing_attachment(target, self).await;
        }
    }

    fn unresolved_reason(&self, target: &AgentRuntimeTarget) -> Option<&'static str> {
        let state = self.lock();
        Some(match state.entries.get(&target.agent_id) {
            Some(entry)
                if entry.runtime_generation != target.runtime_generation
                    || entry.conversation_id != target.conversation_id =>
            {
                "stale_target"
            }
            Some(entry)
                if entry
                    .manager_authority
                    .as_ref()
                    .is_some_and(|owner| !owner.is_current()) =>
            {
                "stale_owner"
            }
            Some(entry) if !entry.published => "unpublished",
            Some(entry) if entry.input.sender().is_none() => "missing_sender",
            Some(_) => return None,
            None if state.staged.contains_key(&target.agent_id) => "unpublished",
            None => "missing_receiver",
        })
    }
}

#[derive(Debug)]
pub(super) enum MissingRecovery {
    Attached,
    Scheduled,
    AlreadyScheduled,
    Superseded,
    NotEligible,
    InspectionParked,
    CapacityDeferred,
    StoreUnavailable,
}

impl DurableAgentConnectionLossHandler {
    pub(super) async fn schedule_missing(
        &self,
        target: &AgentRuntimeTarget,
        registry: &LiveAgentRegistry,
    ) -> MissingRecovery {
        use MissingRecovery::*;
        let current = match self.store.get_agent(&target.agent_id).await {
            Ok(Some(agent)) => agent,
            Ok(None) => return Superseded,
            Err(_) => return StoreUnavailable,
        };
        let Some(candidate) = AgentConnectionRecoveryCandidate::from_agent(&current) else {
            return NotEligible;
        };
        if candidate.conversation_id != target.conversation_id
            || candidate.runtime_generation != target.runtime_generation
        {
            return Superseded;
        }
        // Take the durable predecessor before rechecking the receiver. A
        // concurrent connection publication either satisfies this check or
        // changes the revision and loses the scheduling CAS below.
        let Some(reason) = registry.unresolved_reason(target) else {
            return Attached;
        };
        tracing::info!(agent_id = %target.agent_id, reason,
            "Claimed command attachment unavailable");
        match self.store.agent_inspection_parked(&target.agent_id).await {
            Ok(true) => return InspectionParked,
            Ok(false) => {}
            Err(_) => return StoreUnavailable,
        }
        if candidate.work_step == AgentWorkStep::PublishConnection {
            return AlreadyScheduled;
        }
        if !candidate.recovery_eligible {
            return NotEligible;
        }
        if let Some(recovery) = self.capacity_restart.get() {
            match recovery.route(&candidate.agent_id).await {
                Ok(ReconnectAction::Ordinary) => {}
                Ok(ReconnectAction::Recovered) => return AlreadyScheduled,
                Ok(ReconnectAction::Deferred) | Err(_) => return CapacityDeferred,
            }
        }
        match self
            .store
            .schedule_agent_connection_recovery(candidate.restart_request(now_rfc3339()))
            .await
        {
            Ok(AgentConnectionRecoveryOutcome::Scheduled) => Scheduled,
            Ok(AgentConnectionRecoveryOutcome::AlreadyScheduled) => AlreadyScheduled,
            Ok(AgentConnectionRecoveryOutcome::Superseded) => Superseded,
            Ok(AgentConnectionRecoveryOutcome::NotEligible) => NotEligible,
            Err(SandboxStoreError::Conflict) => Superseded,
            Err(_) => StoreUnavailable,
        }
    }
}
