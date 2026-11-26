//! Authenticate a separate relay capability against fresh provider and custody state.
use super::*;
use crate::auth::persistence::SecretValue;
use layer2_core::sandbox::AgentStatus;

pub(crate) struct AuthenticatedProviderTool {
    pub upstream: url::Url,
    pub credential: SecretValue,
}

impl MongoProviderToolSource {
    pub(crate) async fn authenticate_relay(
        &self,
        capability: &str,
        setup: bool,
    ) -> layer3_adapter::Result<AuthenticatedProviderTool> {
        match self.run(Operation::Authenticate(capability, setup)).await? {
            Outcome::Authenticated(value) => Ok(*value),
            _ => unreachable!("authentication returns current custody"),
        }
    }

    pub(super) async fn relay_auth_in(
        &self,
        capability: &str,
        setup: bool,
        session: &mut ClientSession,
    ) -> transaction::Result<AuthenticatedProviderTool> {
        let authority = self.relay.as_ref().ok_or_else(refused)?;
        let id = authority.verify(capability)?;
        let document = self
            .store
            .bindings
            .tool_relays
            .find_one(doc! {"_id":id})
            .session(&mut *session)
            .await?
            .ok_or_else(refused)?;
        let record = self.read_relay(&document)?;
        let (snapshot, binding) = self.current_relay_lineage(&record.scope, session).await?;
        let lineage = &snapshot.resource.lineage;
        let agent = lineage.agent.as_ref().ok_or_else(refused)?;
        let sandbox = &lineage.sandbox;
        if self
            .standing
            .erasing(&sandbox.owner)
            .await
            .map_err(unavailable)?
        {
            return Err(AdapterError::AccessRevoked.into());
        }
        if record.confirmed {
            let published = matches!(
                agent.status,
                AgentStatus::Ready | AgentStatus::Working | AgentStatus::WaitingApproval
            );
            // Native discovery can follow confirm_open before the worker
            // publishes Ready. A confirmed receipt proves the native identity;
            // actual tool effects additionally require published agent state.
            let restart_open = record.request.restart_runtime.is_some();
            let generation_matches = if restart_open {
                crate::store::agent_tool_open::matches(lineage, &record.request)
            } else {
                agent.conversation.runtime_generation == record.scope.runtime.provision_generation
            };
            let allowed = if setup {
                (published || agent.status == AgentStatus::Starting)
                    && agent
                        .conversation
                        .conversation_id
                        .as_ref()
                        .is_none_or(|id| Some(id) == record.conversation.as_ref())
                    && (generation_matches
                        || !restart_open && agent.conversation.runtime_generation.is_none())
            } else {
                published
                    && agent.conversation.conversation_id == record.conversation
                    && generation_matches
            };
            if !allowed
                || record
                    .scope
                    .runtime
                    .boot_id
                    .as_ref()
                    .is_some_and(|boot| Some(boot) != record.boot_id.as_ref())
            {
                return Err(refused().into());
            }
            let receipt = self
                .store
                .bindings
                .tool_opens
                .find_one(doc! {"_id":record.scope.id()?})
                .session(&mut *session)
                .await?
                .ok_or_else(refused)?;
            let receipt = read(&receipt, &record.scope)?;
            if Some(&receipt.boot_id) != record.boot_id.as_ref()
                || Some(&receipt.conversation) != record.conversation.as_ref()
            {
                return Err(refused().into());
            }
        } else {
            if !setup {
                return Err(refused().into());
            }
            if let Some(sequence) = record.migration_sequence {
                relay_transport::require_intent(
                    &snapshot,
                    record.conversation.as_deref().ok_or_else(refused)?,
                    sequence,
                    &record.origin,
                )?;
            } else {
                // Setup has no external tool effect. The original startup
                // lease and exact request must still own this pending open.
                let conversation=self.store.bindings.conversations.find_one(doc!{"record.agent_id":record.scope.agent_id.as_str(),"record.binding_id":&record.scope.binding_id,"record.borrowed":{"$ne":true}})
                    .session(&mut *session).await?.map(read_conversation).transpose().map_err(unavailable)?.ok_or_else(refused)?;
                let (scope, _) = validation::validate(
                    &snapshot,
                    &binding,
                    &conversation,
                    &RuntimeHandle(record.scope.runtime.runtime_id.clone()),
                    &record.request,
                    record.fence,
                )?;
                if scope != record.scope {
                    return Err(refused().into());
                }
            }
        }
        let current = sandbox.credential.current.as_ref().ok_or_else(refused)?;
        let material = authority
            .custody
            .get_in_session(&sandbox.id, current.generation, session)
            .await
            .map_err(unavailable)?
            .ok_or_else(refused)?;
        if material.sandbox_id != sandbox.id
            || material.owner != sandbox.owner
            || material.authority != *current
            || material.source != sandbox.credential.source
            || !material.matches_source()
        {
            return Err(refused().into());
        }
        // Discovery remains read-only to avoid starving native MCP startup.
        // Every actual tool call fences revocation and retirement before using
        // the current issuer credential, without retrying any remote request.
        if !setup {
            self.fence_relay_lineage(&snapshot, &binding, session)
                .await?;
        }
        Ok(AuthenticatedProviderTool {
            upstream: url::Url::parse(&authority.config.upstream).map_err(|_| refused())?,
            credential: material.value,
        })
    }
}
