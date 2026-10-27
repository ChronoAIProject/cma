use super::*;

impl MongoSandboxStore {
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
        let mut services = services;
        validate_access_services(&services.iter().map(ToString::to_string).collect::<Vec<_>>())?;
        services.sort();
        if services.windows(2).any(|pair| pair[0] == pair[1])
            || !valid_owner_authorization(&authorization)
        {
            return Err(AgentAccessError::Invalid);
        }
        let key_hash = command_hash(key)?;
        transaction!(self, session, {
            let mut record = owned(self, owner, agent, id, session).await?;
            if let Some(AgentAccessDecision::Approved {
                command_key_hash,
                services: previous,
                ..
            }) = &record.decision
            {
                if command_key_hash == &key_hash && previous == &services {
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
            let snapshot = validate_lineage(self, &record.requester, session, false).await?;
            let required = snapshot
                .resource
                .lineage
                .sandbox
                .credential
                .effective_reaches();
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
            record.expected_scope_revision = Some(
                snapshot
                    .resource
                    .lineage
                    .sandbox
                    .credential
                    .service_scope
                    .as_ref()
                    .map_or(0, |scope| scope.revision),
            );
            record.updated_at = layer2_core::now_rfc3339();
            record.approved_at = Some(record.updated_at.clone());
            record.decision = Some(AgentAccessDecision::Approved {
                command_key_hash: key_hash.clone(),
                services: services.clone(),
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
            let snapshot = validate_lineage(self, &record.requester, session, false).await?;
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
            let snapshot = validate_lineage(self, &record.requester, session, false).await?;
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
        .find_one(doc! {"_id":id, "record.requester.owner":owner.as_str(),
        "record.requester.agent_id":agent.as_str()})
        .session(session)
        .await?
        .ok_or(AgentAccessError::NotFound)?;
    Ok(read(&value)?)
}

#[cfg(test)]
#[path = "connection_tests.rs"]
mod tests;
