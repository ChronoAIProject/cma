//! Preserve the runtime and issuer authority while admitting one idle reload.
use super::*;
use layer2_core::sandbox::store::AgentProvenanceSnapshot;
use layer2_core::sandbox::{
    AgentOperationDelivery, AgentOperationIntent, AgentStatus, AgentWorkStep, SandboxStatus,
};

impl MongoProviderToolSource {
    pub(super) async fn current_relay_lineage(
        &self,
        scope: &Scope,
        session: &mut ClientSession,
    ) -> transaction::Result<(AgentProvenanceSnapshot, ProviderBinding)> {
        self.store
            .bindings
            .safety
            .require_associations()
            .map_err(unavailable)?;
        let conversation=self.store.bindings.conversations.find_one(doc! {
            "record.agent_id":scope.agent_id.as_str(), "record.binding_id":&scope.binding_id,
            "record.generation":integer(scope.binding_generation).map_err(unavailable)?, "record.borrowed":{"$ne":true},
        }).session(&mut *session).await?.map(read_conversation).transpose().map_err(unavailable)?.ok_or_else(refused)?;
        let binding = self
            .store
            .bindings
            .records
            .find_one(doc! {"_id":&scope.binding_id})
            .session(&mut *session)
            .await?
            .map(read_binding)
            .transpose()
            .map_err(unavailable)?
            .ok_or_else(refused)?;
        let snapshot =
            crate::store::agent_provenance::read_snapshot(&self.store, &scope.agent_id, session)
                .await?;
        let (mut current, _) = validation::identity(
            &snapshot,
            &binding,
            &conversation,
            &RuntimeHandle(scope.runtime.runtime_id.clone()),
        )?;
        // Settings may legitimately change after installation. The creation,
        // provider, native runtime and pinned profiles must still match exactly.
        current.request_sha256 = scope.request_sha256.clone();
        current.numbered_open.clone_from(&scope.numbered_open);
        if current != *scope {
            return Err(refused().into());
        }
        Ok((snapshot, binding))
    }

    pub(super) async fn fence_relay_lineage(
        &self,
        snapshot: &AgentProvenanceSnapshot,
        binding: &ProviderBinding,
        session: &mut ClientSession,
    ) -> transaction::Result<()> {
        let guarded=self.store.bindings.records.update_one(doc!{"_id":&binding.binding_id,"revision":integer(binding.revision).map_err(unavailable)?},doc!{"$inc":{"tool_open_fence":1_i64}}).session(&mut *session).await?;
        if guarded.matched_count != 1 {
            return Err(refused().into());
        }
        crate::store::resource_access::fence_lineage(&self.store, &snapshot.resource, session)
            .await?;
        Ok(())
    }

    pub(super) async fn relay_transport_in(
        &self,
        attachment: &HostedProviderToolBinding,
        requested_origin: Option<&str>,
        conversation: &str,
        sequence: u64,
        session: &mut ClientSession,
    ) -> transaction::Result<HostedProviderToolBinding> {
        let (runtime, request, fence, proxy) = attachment.scope();
        let document = self
            .store
            .bindings
            .tool_opens
            .find_one(doc! {"_id":attachment.provenance_sha256()})
            .session(&mut *session)
            .await?
            .ok_or_else(refused)?;
        let original: OpenRecord = read_record(&document).map_err(unavailable)?;
        let original = read(&document, &original.scope)?;
        let scope = &original.scope;
        let authority = self.relay_authority(scope)?;
        let (snapshot, binding) = self.current_relay_lineage(scope, session).await?;
        let agent = snapshot
            .resource
            .lineage
            .agent
            .as_ref()
            .ok_or_else(refused)?;
        require_intent(&snapshot, conversation, sequence, &authority.origin)?;
        let observed = attachment.observed().ok_or_else(refused)?;
        observed.validate_for(proxy)?;
        let sandbox = &snapshot.resource.lineage.sandbox;
        let current_proxy = layer3_adapter::ToolProxyRequest {
            sandbox_id: sandbox.id.to_string(),
            generation: scope.runtime.provision_generation.ok_or_else(refused)?,
            endpoint: scope.endpoint.clone(),
            credential_generation: sandbox
                .credential
                .current
                .as_ref()
                .ok_or_else(refused)?
                .generation,
        };
        // Credential rotation invalidates stale proxy observations. Reobserve
        // the original runtime before admitting its descriptor reload.
        observed.validate_for(&current_proxy)?;
        let completed = agent.work.lease.is_none() && agent.work.step == AgentWorkStep::None;
        let recovering = agent.work.step == AgentWorkStep::PublishConnection
            && agent.work.lease.as_ref().is_some_and(|lease| {
                lease.fence == fence
                    && layer2_core::seconds_from_rfc3339(&lease.expires_at).is_some_and(|expiry| {
                        std::time::UNIX_EPOCH + std::time::Duration::from_secs(expiry)
                            > std::time::SystemTime::now()
                    })
            });
        if original.conversation != conversation
            || original.boot_id != observed.boot_id
            || scope.runtime.runtime_id != runtime.0
            || !scope.matches_open(request)
            || scope
                .runtime
                .boot_id
                .as_ref()
                .is_some_and(|boot| boot != &observed.boot_id)
            || !(completed || recovering)
            || fence.checked_add(1) != Some(agent.work.next_fence)
            || requested_origin.is_some_and(|origin| {
                url::Url::parse(origin).ok().as_ref().map(url::Url::as_str)
                    != Some(authority.origin.as_str())
            })
        {
            return Err(refused().into());
        }
        let migrated = self.relay_binding(attachment.clone(), scope)?;
        if requested_origin.is_none() && !migrated.same_transport(attachment) {
            return Err(refused().into());
        }
        let existing = self.relay_record(scope, session).await?;
        if let Some(previous) = &existing {
            if previous
                .conversation
                .as_deref()
                .is_some_and(|id| id != conversation)
                || previous
                    .boot_id
                    .as_deref()
                    .is_some_and(|boot| boot != observed.boot_id)
            {
                return Err(refused().into());
            }
        }
        if requested_origin.is_none()
            && existing.as_ref().is_none_or(|record| {
                record.migration_sequence != Some(sequence) || record.fence != fence
            })
        {
            return Err(refused().into());
        }
        if attachment.transport_origin().is_some() && !migrated.same_transport(attachment) {
            return Err(refused().into());
        }
        let record = relay::Record {
            scope: scope.clone(),
            request: request.clone(),
            fence,
            origin: authority.origin.clone(),
            conversation: Some(conversation.into()),
            boot_id: Some(observed.boot_id.clone()),
            confirmed: requested_origin.is_none()
                || existing.as_ref().is_some_and(|record| record.confirmed),
            migration_sequence: Some(sequence),
        };
        self.fence_relay_lineage(&snapshot, &binding, session)
            .await?;
        self.save_relay(&record, session).await?;
        Ok(migrated)
    }
}

pub(super) fn require_intent(
    snapshot: &AgentProvenanceSnapshot,
    conversation: &str,
    sequence: u64,
    origin: &str,
) -> layer3_adapter::Result<()> {
    let agent = snapshot
        .resource
        .lineage
        .agent
        .as_ref()
        .ok_or_else(refused)?;
    let operation = agent.conversation.operation.as_ref().ok_or_else(refused)?;
    let AgentOperationIntent::RefreshContext {
        service_origin: Some(target),
        ..
    } = &operation.intent
    else {
        return Err(refused());
    };
    if sequence == 0
        || operation.sequence != sequence
        || url::Url::parse(target).ok().as_ref().map(url::Url::as_str) != Some(origin)
        || !matches!(
            operation.state,
            AgentOperationDelivery::DispatchFenced
                | AgentOperationDelivery::RecoveryRequired
                | AgentOperationDelivery::WriteConfirmed
        )
        || agent.status != AgentStatus::Ready
        || agent.conversation.active_turn.is_some()
        || agent.conversation.conversation_id.as_deref() != Some(conversation)
        || agent.conversation.runtime_generation != Some(operation.runtime_generation)
        || snapshot.resource.lineage.sandbox.status != SandboxStatus::Active
    {
        return Err(refused());
    }
    Ok(())
}
