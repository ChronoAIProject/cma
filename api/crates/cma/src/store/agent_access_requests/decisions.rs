use super::*;
use layer2_core::sandbox::agent_access::GithubApprovalContext;
use layer2_core::sandbox::ServiceConnectionSelection;

impl MongoSandboxStore {
    pub(crate) async fn github_approval_context(
        &self,
        owner: &SubjectId,
        agent: &AgentId,
    ) -> Result<GithubApprovalContext, AgentAccessError> {
        transaction!(self, session, {
            let snapshot = crate::store::agent_provenance::read_snapshot(self, agent, session)
                .await
                .map_err(provenance_error)?;
            let lineage = &snapshot.resource.lineage;
            if lineage
                .agent
                .as_ref()
                .is_none_or(|agent| agent.owner != *owner)
                || lineage.sandbox.owner != *owner
            {
                return Err(AgentAccessError::NotFound.into());
            }
            let sandbox = &lineage.sandbox;
            let mut cursor = self
                .agents
                .find(doc! {
                    "sandbox_id": sandbox.id.as_str(),
                    "owner": owner.as_str(),
                    "_id": {"$ne": agent.as_str()},
                    "status": {"$ne": "deleted"},
                })
                .sort(doc! {"_id": 1})
                .projection(doc! {"_id": 1})
                .limit(201)
                .session(&mut *session)
                .await?;
            let mut other_agents = Vec::new();
            while let Some(row) = cursor.next(&mut *session).await {
                let id = row?
                    .get_str("_id")
                    .map_err(|_| AgentAccessError::Unavailable)?
                    .to_owned();
                other_agents.push(AgentId(id));
            }
            if other_agents.len() > 200 {
                return Err(AgentAccessError::Unavailable.into());
            }
            let scope = sandbox.credential.service_scope.as_ref();
            Ok(GithubApprovalContext {
                sandbox_id: sandbox.id.clone(),
                sandbox_name: sandbox.name.to_string(),
                other_agents,
                current_user_service_id: scope
                    .filter(|scope| {
                        sandbox.credential.delivered
                            && sandbox.credential.current.as_ref().is_some_and(|current| {
                                current.generation > scope.credential_generation_floor
                            })
                    })
                    .and_then(|scope| scope.connection.as_ref())
                    .filter(|connection| {
                        connection.service.as_str() == crate::trigger::GITHUB_SERVICE
                    })
                    .map(|connection| connection.connection_id.clone()),
                scope_revision: scope.map_or(0, |scope| scope.revision),
                credential_generation: sandbox.credential.key_generation,
            })
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn github_approval_replay(
        &self,
        owner: &SubjectId,
        agent: &AgentId,
        id: &str,
        services: &[ServiceId],
        connection: &ServiceConnectionSelection,
        binding: &ConnectionApprovalBinding,
        key: &str,
    ) -> Result<Option<AgentAccessRecord>, AgentAccessError> {
        let key_hash = command_hash(key)?;
        let row = self
            .agent_access_requests
            .find_one(approval_filter(owner, agent, id))
            .await
            .map_err(|_| AgentAccessError::Unavailable)?
            .ok_or(AgentAccessError::NotFound)?;
        let record = read(&row)?;
        if record.requester.sandbox_id != binding.sandbox_id {
            return Err(AgentAccessError::Conflict);
        }
        match &record.decision {
            Some(AgentAccessDecision::Approved {
                command_key_hash,
                services: previous,
                connection: Some(previous_connection),
                connection_binding: Some(previous_binding),
                ..
            }) if command_key_hash == &key_hash
                && previous == services
                && previous_connection == connection
                && previous_binding == binding =>
            {
                Ok(Some(record))
            }
            None if record.phase == AgentAccessPhase::Pending => Ok(None),
            _ => Err(AgentAccessError::Conflict),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn approve_agent_access(
        &self,
        owner: &SubjectId,
        agent: &AgentId,
        id: &str,
        services: Vec<ServiceId>,
        baseline: &[ServiceId],
        authorization: AgentAuthorizationRevision,
        key: &str,
    ) -> Result<AgentAccessRecord, AgentAccessError> {
        self.approve_agent_access_with_connection(
            owner,
            agent,
            id,
            services,
            None,
            baseline,
            authorization,
            key,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn approve_agent_access_with_connection(
        &self,
        owner: &SubjectId,
        agent: &AgentId,
        id: &str,
        services: Vec<ServiceId>,
        connection: Option<ServiceConnectionSelection>,
        baseline: &[ServiceId],
        authorization: AgentAuthorizationRevision,
        key: &str,
    ) -> Result<AgentAccessRecord, AgentAccessError> {
        self.approve_agent_access_bound(
            owner,
            agent,
            id,
            services,
            connection,
            None,
            baseline,
            authorization,
            key,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn approve_agent_access_bound(
        &self,
        owner: &SubjectId,
        agent: &AgentId,
        id: &str,
        services: Vec<ServiceId>,
        connection: Option<ServiceConnectionSelection>,
        binding: Option<ConnectionApprovalBinding>,
        baseline: &[ServiceId],
        authorization: AgentAuthorizationRevision,
        key: &str,
    ) -> Result<AgentAccessRecord, AgentAccessError> {
        let mut services = services;
        validate_access_services(&services.iter().map(ToString::to_string).collect::<Vec<_>>())?;
        services.sort();
        if services.windows(2).any(|pair| pair[0] == pair[1])
            || !valid_owner_authorization(&authorization)
            || (binding.is_some() && connection.is_none())
            || connection
                .as_ref()
                .is_some_and(|selection| !selection.matches_scope(&services))
        {
            return Err(AgentAccessError::Invalid);
        }
        let key_hash = command_hash(key)?;
        transaction!(self, session, {
            let mut record = owned(self, owner, agent, id, session).await?;
            if let Some(AgentAccessDecision::Approved {
                command_key_hash,
                services: previous,
                connection: previous_connection,
                connection_binding: previous_binding,
                ..
            }) = &record.decision
            {
                if command_key_hash == &key_hash
                    && previous == &services
                    && previous_connection == &connection
                    && previous_binding == &binding
                {
                    return Ok(record);
                }
                return Err(AgentAccessError::Conflict.into());
            }
            if record.phase != AgentAccessPhase::Pending {
                return Err(AgentAccessError::Conflict.into());
            }
            if layer2_core::seconds_from_rfc3339(&record.created_at).is_none_or(|created| {
                layer2_core::seconds_from_rfc3339(&layer2_core::now_rfc3339())
                    .is_none_or(|now| now.saturating_sub(created) >= 24 * 3600)
            }) {
                return Err(AgentAccessError::Conflict.into());
            }
            let snapshot =
                validate_lineage_transaction(self, &record.requester, session, false).await?;
            let sandbox = &snapshot.resource.lineage.sandbox;
            let scope_revision = sandbox
                .credential
                .service_scope
                .as_ref()
                .map_or(0, |scope| scope.revision);
            if binding.as_ref().is_some_and(|binding| {
                binding.sandbox_id != sandbox.id
                    || binding.scope_revision != scope_revision
                    || binding.credential_generation != sandbox.credential.key_generation
            }) {
                // No decision exists in this transaction, and no write has
                // been attempted. The owner must approve the new binding.
                return Err(AgentAccessError::StaleGithubApproval.into());
            }
            let required = sandbox.credential.effective_reaches();
            if !baseline
                .iter()
                .chain(required.iter())
                .all(|value| services.contains(value))
                || !record
                    .proposal
                    .services
                    .iter()
                    .all(|value| services.iter().any(|service| service.as_str() == value))
            {
                return Err(AgentAccessError::Invalid.into());
            }
            fence(self, &snapshot, session).await?;
            let revision = record.revision;
            record.revision += 1;
            record.phase = AgentAccessPhase::Connecting;
            record.expected_scope_revision = Some(scope_revision);
            record.updated_at = layer2_core::now_rfc3339();
            record.approved_at = Some(record.updated_at.clone());
            record.decision = Some(AgentAccessDecision::Approved {
                command_key_hash: key_hash.clone(),
                services: services.clone(),
                connection: connection.clone(),
                connection_binding: binding.clone(),
                authorization: authorization.clone(),
            });
            replace(self, revision, &record, session).await?;
            Ok(record)
        })
    }

    pub(crate) async fn deny_agent_access(
        &self,
        owner: &SubjectId,
        agent: &AgentId,
        id: &str,
        key: &str,
    ) -> Result<AgentAccessRecord, AgentAccessError> {
        let key_hash = command_hash(key)?;
        transaction!(self, session, {
            let mut record = owned(self, owner, agent, id, session).await?;
            if let Some(AgentAccessDecision::Denied { command_key_hash }) = &record.decision {
                if command_key_hash == &key_hash {
                    return Ok(record);
                }
                return Err(AgentAccessError::Conflict.into());
            }
            if !matches!(
                record.phase,
                AgentAccessPhase::Pending | AgentAccessPhase::Connecting
            ) {
                return Err(AgentAccessError::Conflict.into());
            }
            let snapshot =
                validate_lineage_transaction(self, &record.requester, session, false).await?;
            fence(self, &snapshot, session).await?;
            let revision = record.revision;
            record.revision += 1;
            record.phase = AgentAccessPhase::Denied;
            record.lease = None;
            record.approved_at = None;
            record.updated_at = layer2_core::now_rfc3339();
            record.decision = Some(AgentAccessDecision::Denied {
                command_key_hash: key_hash.clone(),
            });
            replace(self, revision, &record, session).await?;
            Ok(record)
        })
    }

    pub(crate) async fn access_request_connection_services(
        &self,
        owner: &SubjectId,
        id: &str,
    ) -> Result<Vec<ServiceId>, AgentAccessError> {
        transaction!(self, session, {
            let value = self
                .agent_access_requests
                .find_one(doc! {"_id":id, "record.requester.owner":owner.as_str()})
                .session(&mut *session)
                .await?
                .ok_or(AgentAccessError::NotFound)?;
            let record = read(&value)?;
            if record.phase != AgentAccessPhase::Connecting && !record.phase.applying() {
                return Err(AgentAccessError::Conflict.into());
            }
            let snapshot =
                validate_lineage_transaction(self, &record.requester, session, false).await?;
            fence(self, &snapshot, session).await?;
            Ok(record.approved_services()?.to_vec())
        })
    }
}

async fn owned(
    store: &MongoSandboxStore,
    owner: &SubjectId,
    agent: &AgentId,
    id: &str,
    session: &mut ClientSession,
) -> TxResult<AgentAccessRecord> {
    let value = store
        .agent_access_requests
        .find_one(approval_filter(owner, agent, id))
        .session(session)
        .await?
        .ok_or(AgentAccessError::NotFound)?;
    Ok(read(&value)?)
}

fn approval_filter(owner: &SubjectId, agent: &AgentId, id: &str) -> mongodb::bson::Document {
    doc! {
        "_id": id,
        "record.requester.owner": owner.as_str(),
        "record.requester.agent_id": agent.as_str(),
    }
}

#[cfg(test)]
#[path = "connection_tests.rs"]
mod tests;
