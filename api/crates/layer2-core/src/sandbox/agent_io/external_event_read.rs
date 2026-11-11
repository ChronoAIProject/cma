use super::{
    admission_service::bounded, external_event_key, AgentCommandService, CommandAdmissionError,
    CommandLedgerError, ExternalEventContext,
};
use crate::{
    resource_access::{AccessPurpose, CallerContext, ResourceCapability, ResourceTarget},
    AgentId,
};
use std::time::Instant;

const EVENT_CONTEXT_TIMING_TARGET: &str = "cma::responses::event_context";

fn log_phase(phase: &'static str, started: Instant) {
    tracing::info!(
        target: EVENT_CONTEXT_TIMING_TARGET,
        phase,
        elapsed_ms = started.elapsed().as_millis() as u64,
        "retained event context phase"
    );
}

struct TotalTiming(Instant);

impl Drop for TotalTiming {
    fn drop(&mut self) {
        log_phase("total", self.0);
    }
}

impl AgentCommandService {
    /// Locate immutable transport context independently of provider-owned affinity.
    /// Authorization is applied to the actual retained Agent before returning data.
    pub async fn external_event_context_for_source(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        binding: &str,
        conversation: &str,
        event: &str,
    ) -> Result<ExternalEventContext, CommandAdmissionError> {
        let _total = TotalTiming(Instant::now());
        let key = external_event_key(&format!("binding:{binding}"), conversation, event);
        let lookup_started = Instant::now();
        let record = self.store.get_external_event_command(&key).await;
        log_phase("retained_command_lookup", lookup_started);
        let record = record?.ok_or(CommandLedgerError::NotFound)?;
        self.external_event_context(
            context,
            credential,
            &record.receipt.envelope.agent_id,
            binding,
            conversation,
            event,
        )
        .await
    }

    /// Read the retained command once, then carry that immutable record through
    /// current authorization and the final release fence. This is deliberately
    /// called only by the explicit rollout flag; the legacy path above remains
    /// the default until production evidence accepts the optimization.
    pub async fn external_event_context_for_source_buffered(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        binding: &str,
        conversation: &str,
        event: &str,
    ) -> Result<ExternalEventContext, CommandAdmissionError> {
        let _total = TotalTiming(Instant::now());
        let key = external_event_key(&format!("binding:{binding}"), conversation, event);

        let lookup_started = Instant::now();
        let record = self.store.get_external_event_command(&key).await;
        log_phase("retained_command_lookup", lookup_started);
        let record = record?.ok_or(CommandLedgerError::NotFound)?;
        record.validate()?;
        let agent = record.receipt.envelope.agent_id.clone();

        let target = ResourceTarget::Agent { agent_id: agent };
        let capability = ResourceCapability::ViewHistory;
        let authorization_started = Instant::now();
        let authorized = bounded(self.access.authorize(
            context,
            credential,
            &target,
            capability,
            AccessPurpose::Read,
        ))
        .await;
        log_phase("resource_authorization", authorization_started);
        let authorized = authorized??;
        let proof = authorized
            .binding_authority()
            .ok_or(CommandLedgerError::NotFound)?;
        if proof.binding.binding_id != binding {
            return Err(CommandLedgerError::NotFound.into());
        }

        // Validate the buffered record against the current proof before the
        // final fence. No second command lookup is permitted on this path.
        proof.require_command(&record.receipt.envelope)?;
        if record.receipt.envelope.external_event_key().as_ref() != Some(&key) {
            return Err(CommandLedgerError::Corrupt.into());
        }
        let context = record
            .receipt
            .envelope
            .command
            .external_event()
            .cloned()
            .ok_or(CommandLedgerError::Corrupt)?;

        let fence_started = Instant::now();
        let fence = self.fence(&authorized, &target, capability).await;
        log_phase("final_release_fence", fence_started);
        fence?;
        Ok(context)
    }
    /// Only current binding authority can read its own retained external context.
    /// Revalidate after buffering so rotation or retirement can revoke the read.
    pub async fn external_event_context(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        agent: &AgentId,
        binding: &str,
        conversation: &str,
        event: &str,
    ) -> Result<ExternalEventContext, CommandAdmissionError> {
        let target = ResourceTarget::Agent {
            agent_id: agent.clone(),
        };
        let capability = ResourceCapability::ViewHistory;
        let authorization_started = Instant::now();
        let authorized = bounded(self.access.authorize(
            context,
            credential,
            &target,
            capability,
            AccessPurpose::Read,
        ))
        .await;
        log_phase("resource_authorization", authorization_started);
        let authorized = authorized??;
        let proof = authorized
            .binding_authority()
            .ok_or(CommandLedgerError::NotFound)?;
        if proof.binding.binding_id != binding {
            return Err(CommandLedgerError::NotFound.into());
        }
        let key = external_event_key(&format!("binding:{binding}"), conversation, event);
        let command_started = Instant::now();
        let result = self
            .with_authority_bound(
                &authorized,
                capability,
                self.store.get_external_event_command(&key),
            )
            .await;
        log_phase("retained_command_recheck", command_started);
        let fence_started = Instant::now();
        let fence = self.fence(&authorized, &target, capability).await;
        log_phase("final_release_fence", fence_started);
        fence?;
        let record = result??.ok_or(CommandLedgerError::NotFound)?;
        record.validate()?;
        proof.require_command(&record.receipt.envelope)?;
        if record.receipt.envelope.external_event_key().as_ref() != Some(&key) {
            return Err(CommandLedgerError::Corrupt.into());
        }
        record
            .receipt
            .envelope
            .command
            .external_event()
            .cloned()
            .ok_or(CommandLedgerError::Corrupt.into())
    }
}
