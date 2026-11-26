use super::*;
use layer2_core::sandbox::store::AgentProvenanceSnapshot;

impl MongoAgentAccessToolSource {
    pub(super) async fn setup_in(
        &self,
        capability: &str,
        session: &mut ClientSession,
    ) -> Result<()> {
        let document = self
            .store
            .agent_access_tools
            .find_one(doc! {"_id": records::hash(capability)})
            .session(&mut *session)
            .await?
            .ok_or(Error::Refused)?;
        let record = self.read(&document)?;
        let snapshot = super::super::agent_provenance::read_snapshot(
            &self.store,
            &record.scope.creation.agent_id,
            session,
        )
        .await?;
        let scope = validation::scope(
            &snapshot,
            &RuntimeHandle(record.scope.runtime.runtime_id.clone()),
        )?;
        self.require_owner_available(&snapshot.resource.lineage.sandbox.owner)
            .await?;
        if scope != record.scope {
            return Err(Error::Refused);
        }
        if record.conversation.is_none() {
            validation::request(&snapshot, &record.request, record.fence)?;
        } else {
            require_conversation(&snapshot, record.conversation.as_deref(), false)?;
            if record.request.restart_runtime.is_some()
                && !crate::store::agent_tool_open::matches(
                    &snapshot.resource.lineage,
                    &record.request,
                )
            {
                return Err(Error::Refused);
            }
        }
        // Discovery returns no Agent identity and cannot admit a grant or a
        // command. The transaction's snapshot proves the current receipt,
        // lineage and runtime without writing to the concurrently publishing
        // Agent. A write fence here starves MCP initialization as first-turn
        // updates advance that same lineage. Actual tool calls and admission
        // retain their mutation fences below and in the consent store.
        Ok(())
    }

    pub(super) async fn resolve_in(
        &self,
        runtime: &RuntimeHandle,
        request: &AgentOpenRequest,
        fence: u64,
        session: &mut ClientSession,
    ) -> Result<Option<HostedAccessToolBinding>> {
        let snapshot = super::super::agent_provenance::read_snapshot(
            &self.store,
            &AgentId(request.options.slot.0.clone()),
            session,
        )
        .await?;
        // Supplied keys belong to their issuer. CMA cannot revise their scope,
        // and their retained expiry is intentionally unknown. This optional
        // consent tool must not prevent a provider-bound Agent from opening;
        // its required Event Gateway tools have separate binding authority.
        if !snapshot
            .resource
            .lineage
            .sandbox
            .credential
            .source
            .is_managed()
        {
            return Ok(None);
        }
        let agent = snapshot
            .resource
            .lineage
            .agent
            .as_ref()
            .ok_or(Error::Refused)?;
        // A deployment cannot retrofit MCP onto an unproven live conversation.
        // Enrollment survives worker takeover and permits fresh session/load on
        // a replacement runtime, but never grants an Existing bridge a receipt.
        if !validation::new_agent(agent) {
            if self
                .store
                .agent_access_tools
                .find_one(doc! {
                    "agent_id": agent.id.as_str(), "record.conversation": {"$type": "string"},
                })
                .session(&mut *session)
                .await?
                .is_none()
            {
                return Ok(None);
            }
            let previous = self
                .store
                .agent_access_tools
                .find_one(doc! {
                    "agent_id": agent.id.as_str(), "record.conversation": {"$type": "string"},
                    "record.scope.creation": record_bson(&snapshot.creation_provenance(&agent.id)
                        .map_err(|_| Error::Refused)?).map_err(|_| Error::Unavailable)?,
                })
                .session(&mut *session)
                .await?;
            if previous.is_none() {
                return Ok(None);
            }
            self.read(&previous.expect("existing enrollment was checked"))?;
        }
        let scope = validation::scope(&snapshot, runtime)?;
        validation::request(&snapshot, request, fence)?;
        let previous = self.current_record(&scope, session).await?;
        let authority = previous.as_ref().map_or(0, |record| record.authority);
        let capability = self.capability_at(&scope, authority)?;
        let transport_origin = previous
            .as_ref()
            .and_then(|record| record.transport_origin.clone());
        let origin = transport_origin
            .as_deref()
            .unwrap_or(self.authority(authority)?.0);
        let conversation = previous.and_then(|record| record.conversation);
        require_conversation(&snapshot, conversation.as_deref(), false)?;
        let mut binding = HostedAccessToolBinding::new(
            origin,
            capability.into(),
            runtime.clone(),
            request,
            fence,
            conversation.clone(),
        )
        .map_err(|_| Error::Refused)?;
        if conversation.is_none() {
            if let Some(prepared) =
                super::super::native_prepared::prepared_conversation_transaction(
                    &self.store,
                    &snapshot,
                    request,
                    session,
                )
                .await?
            {
                binding = binding
                    .with_prepared_conversation(&prepared)
                    .map_err(|_| Error::Refused)?;
            }
        }
        super::super::resource_access::fence_lineage(&self.store, &snapshot.resource, session)
            .await?;
        self.save(
            &OpenRecord {
                authority,
                transport_origin,
                scope,
                request: request.clone(),
                fence,
                conversation,
            },
            session,
        )
        .await?;
        Ok(Some(binding))
    }

    pub(super) async fn confirm_in(
        &self,
        binding: &HostedAccessToolBinding,
        conversation: &str,
        session: &mut ClientSession,
    ) -> Result<()> {
        let (runtime, slot, request_sha256, fence) = binding.scope();
        let snapshot = super::super::agent_provenance::read_snapshot(
            &self.store,
            &AgentId(slot.0.clone()),
            session,
        )
        .await?;
        let scope = validation::scope(&snapshot, runtime)?;
        let mut record = self
            .current_record(&scope, session)
            .await?
            .ok_or(Error::Refused)?;
        let capability = self.capability_at(&scope, record.authority)?;
        let origin = self.transport_origin(&record)?;
        validation::request(&snapshot, &record.request, fence)?;
        let mut expected = HostedAccessToolBinding::new(
            origin,
            capability.clone().into(),
            runtime.clone(),
            &record.request,
            fence,
            record.conversation.clone(),
        )
        .map_err(|_| Error::Refused)?;
        let mut initial = HostedAccessToolBinding::new(
            origin,
            capability.into(),
            runtime.clone(),
            &record.request,
            fence,
            None,
        )
        .map_err(|_| Error::Refused)?;
        if let Some(prepared) = super::super::native_prepared::prepared_conversation_transaction(
            &self.store,
            &snapshot,
            &record.request,
            session,
        )
        .await?
        {
            if conversation != prepared {
                return Err(Error::Refused);
            }
            initial = initial
                .with_prepared_conversation(&prepared)
                .map_err(|_| Error::Refused)?;
            if record.conversation.is_none() {
                expected = expected
                    .with_prepared_conversation(&prepared)
                    .map_err(|_| Error::Refused)?;
            }
        }
        // Comparing scope and the exact source-derived binding also excludes a
        // caller-constructed binding carrying another endpoint or capability.
        if record.scope != scope
            || record.fence != fence
            || record.request.request_sha256 != request_sha256
            || (&expected != binding && &initial != binding)
            || record
                .conversation
                .as_deref()
                .is_some_and(|value| value != conversation)
        {
            return Err(Error::Refused);
        }
        require_conversation(&snapshot, Some(conversation), false)?;
        super::super::resource_access::fence_lineage(&self.store, &snapshot.resource, session)
            .await?;
        record.conversation = Some(conversation.into());
        self.save(&record, session).await
    }

    pub(super) async fn authenticate_in(
        &self,
        capability: &str,
        session: &mut ClientSession,
    ) -> Result<AuthenticatedAgentAccess> {
        let document = self
            .store
            .agent_access_tools
            .find_one(doc! {"_id": records::hash(capability)})
            .session(&mut *session)
            .await?
            .ok_or(Error::Refused)?;
        let record = self.read(&document)?;
        let snapshot = super::super::agent_provenance::read_snapshot(
            &self.store,
            &record.scope.creation.agent_id,
            session,
        )
        .await?;
        let scope = validation::scope(
            &snapshot,
            &RuntimeHandle(record.scope.runtime.runtime_id.clone()),
        )?;
        self.require_owner_available(&snapshot.resource.lineage.sandbox.owner)
            .await?;
        if record.scope != scope {
            return Err(Error::Refused);
        }
        let conversation = record.conversation.ok_or(Error::Refused)?;
        require_conversation(&snapshot, Some(&conversation), true)?;
        if !crate::store::agent_tool_open::published(&snapshot.resource.lineage, &record.request) {
            return Err(Error::Refused);
        }
        super::super::resource_access::fence_lineage(&self.store, &snapshot.resource, session)
            .await?;
        Ok(AuthenticatedAgentAccess {
            agent_id: scope.creation.agent_id.clone(),
            owner: snapshot.resource.lineage.sandbox.owner.clone(),
            sandbox_id: scope.creation.sandbox_id.clone(),
            conversation,
            runtime: scope.runtime,
            creation: scope.creation,
        })
    }
}

fn require_conversation(
    snapshot: &AgentProvenanceSnapshot,
    conversation: Option<&str>,
    published: bool,
) -> Result<()> {
    let agent = snapshot
        .resource
        .lineage
        .agent
        .as_ref()
        .ok_or(Error::Refused)?;
    if conversation.is_some_and(|value| {
        agent
            .conversation
            .conversation_id
            .as_deref()
            .is_some_and(|id| id != value)
            || (published && agent.conversation.conversation_id.as_deref() != Some(value))
    }) {
        return Err(Error::Refused);
    }
    Ok(())
}
