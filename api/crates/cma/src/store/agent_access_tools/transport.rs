//! Retain signing authority while confirming a fenced idle transport change.
use super::*;
use layer2_core::sandbox::{
    AgentOperationDelivery, AgentOperationIntent, AgentStatus, SandboxStatus,
};

impl MongoAgentAccessToolSource {
    pub(super) async fn transport_in(
        &self,
        binding: &HostedAccessToolBinding,
        requested_origin: Option<&str>,
        conversation: &str,
        sequence: u64,
        session: &mut ClientSession,
    ) -> Result<HostedAccessToolBinding> {
        let (runtime, slot, request_hash, fence) = binding.scope();
        let snapshot = super::super::agent_provenance::read_snapshot(
            &self.store,
            &AgentId(slot.0.clone()),
            session,
        )
        .await?;
        let scope = validation::scope(&snapshot, runtime)?;
        let agent = snapshot
            .resource
            .lineage
            .agent
            .as_ref()
            .ok_or(Error::Refused)?;
        let operation = agent
            .conversation
            .operation
            .as_ref()
            .ok_or(Error::Refused)?;
        let AgentOperationIntent::RefreshContext {
            service_origin: Some(target),
            ..
        } = &operation.intent
        else {
            return Err(Error::Refused);
        };
        let target = url::Url::parse(target)
            .map_err(|_| Error::Refused)?
            .to_string();
        if sequence == 0
            || operation.sequence != sequence
            || !matches!(
                operation.state,
                AgentOperationDelivery::DispatchFenced
                    | AgentOperationDelivery::RecoveryRequired
                    | AgentOperationDelivery::WriteConfirmed
            )
            || agent.status != AgentStatus::Ready
            || agent.conversation.active_turn.is_some()
            || agent.conversation.runtime_generation != Some(operation.runtime_generation)
            || snapshot.resource.lineage.sandbox.status != SandboxStatus::Active
            || !validation::valid_conversation(conversation)
            || agent.conversation.conversation_id.as_deref() != Some(conversation)
            || !self.records.recognizes_origin(&target)
            || requested_origin.is_some_and(|origin| {
                url::Url::parse(origin)
                    .map(|u| u.to_string())
                    .ok()
                    .as_deref()
                    != Some(target.as_str())
            })
        {
            return Err(Error::Refused);
        }
        let mut record = self
            .current_record(&scope, session)
            .await?
            .ok_or(Error::Refused)?;
        validation::retained_request(&snapshot, &record.request, fence)?;
        if record.scope != scope
            || record.fence != fence
            || record.request.request_sha256 != request_hash
            || record.conversation.as_deref() != Some(conversation)
        {
            return Err(Error::Refused);
        }
        let capability = self.capability_at(&scope, record.authority)?;
        let make = |origin: &str| {
            HostedAccessToolBinding::new(
                origin,
                capability.clone().into(),
                runtime.clone(),
                &record.request,
                fence,
                Some(conversation.into()),
            )
            .map_err(|_| Error::Refused)
        };
        let current = make(self.transport_origin(&record)?)?;
        let migrated = make(&target)?;
        if requested_origin.is_some() {
            if binding != &current && binding != &migrated {
                return Err(Error::Refused);
            }
        } else {
            if binding != &migrated {
                return Err(Error::Refused);
            }
            // Store the exact transport only after acknowledged native reload.
            // The capability hash/signing namespace and conversation are unchanged.
            super::super::resource_access::fence_lineage(&self.store, &snapshot.resource, session)
                .await?;
            record.transport_origin = Some(target);
            self.save(&record, session).await?;
        }
        Ok(migrated)
    }
}
