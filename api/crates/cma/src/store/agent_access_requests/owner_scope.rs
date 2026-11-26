//! A direct owner Save is already consent; retries retain its exact desired set.
use super::*;
use layer2_core::sandbox::SandboxStatus;

impl MongoSandboxStore {
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn replace_agent_service_access(
        &self,
        owner: &SubjectId,
        agent_id: &AgentId,
        mut services: Vec<ServiceId>,
        mut expected_services: Vec<ServiceId>,
        baseline: &[ServiceId],
        authorization: AgentAuthorizationRevision,
        key: &str,
    ) -> Result<AgentAccessRecord, AgentAccessError> {
        for values in [&mut services, &mut expected_services] {
            validate_access_services(&values.iter().map(ToString::to_string).collect::<Vec<_>>())?;
            values.sort();
            if values.windows(2).any(|pair| pair[0] == pair[1]) {
                return Err(AgentAccessError::Invalid);
            }
        }
        if !baseline.iter().all(|service| services.contains(service))
            || !valid_owner_authorization(&authorization)
        {
            return Err(AgentAccessError::Invalid);
        }
        let key_hash = command_hash(key)?;
        let now = layer2_core::now_rfc3339();
        let mut observed_identity = None;
        let mut original_intent = None;
        transaction!(self, session, {
            let snapshot = crate::store::agent_provenance::read_snapshot(self, agent_id, session)
                .await
                .map_err(|error| match error {
                    crate::store::resource_access::ReadError::Driver(error) => Error::Driver(error),
                    _ => Error::Domain(AgentAccessError::NotFound),
                })?;
            let sandbox = &snapshot.resource.lineage.sandbox;
            let agent = snapshot
                .resource
                .lineage
                .agent
                .as_ref()
                .ok_or(AgentAccessError::NotFound)?;
            let creation = snapshot
                .creation_provenance(agent_id)
                .map_err(|_| AgentAccessError::NotFound)?;
            if &agent.owner != owner
                || &sandbox.owner != owner
                || !agent.status.is_live()
                || agent.requested_stop_reason.is_some()
                || sandbox.status == SandboxStatus::Retired
                || !sandbox.credential.source.is_managed()
                || !matches!(&creation.owner, layer2_core::resource_access::ResourceOwner::Person { subject } if subject == owner)
            {
                return Err(AgentAccessError::NotFound.into());
            }
            let requester = AgentAccessRequester {
                agent_id: agent.id.clone(),
                owner: owner.clone(),
                sandbox_id: sandbox.id.clone(),
                conversation: agent
                    .conversation
                    .conversation_id
                    .clone()
                    .ok_or(AgentAccessError::Conflict)?,
                runtime: sandbox
                    .runtime
                    .as_ref()
                    .or(sandbox.lifecycle.suspended_runtime.as_deref())
                    .cloned()
                    .ok_or(AgentAccessError::Conflict)?,
                creation,
            };
            let scope_revision = sandbox
                .credential
                .service_scope
                .as_ref()
                .map_or(0, |scope| scope.revision);
            let identity = (requester.clone(), scope_revision);
            if observed_identity
                .as_ref()
                .is_some_and(|original| original != &identity)
            {
                return Err(AgentAccessError::Conflict.into());
            }
            observed_identity = Some(identity);
            let proposal = AgentAccessProposal {
                request_key: format!("owner:{key_hash}"),
                retry_of: None,
                services: services.iter().map(ToString::to_string).collect(),
                reason: "Update the service scope shared by every Agent in this Sandbox.".into(),
            };
            proposal.validate()?;
            let id = records::request_id(&requester, &proposal.request_key)?;
            let body_sha256 = records::digest(&proposal)?;
            let mode = AgentAccessScopeMode::Replace {
                expected_services: expected_services.clone(),
            };
            if let Some(value) = self
                .agent_access_requests
                .find_one(doc! {"_id":&id})
                .session(&mut *session)
                .await?
            {
                let previous = read(&value)?;
                if previous.body_sha256 != body_sha256
                    || previous.scope_mode != mode
                    || previous.requester.creation != requester.creation
                    || previous.requester.conversation != requester.conversation
                {
                    return Err(AgentAccessError::Conflict.into());
                }
                return Ok(previous);
            }
            if sandbox.credential.effective_reaches() != expected_services {
                return Err(AgentAccessError::Conflict.into());
            }
            if self
                .agent_access_requests
                .count_documents(doc! {
                    "record.requester.agent_id":agent_id.as_str(), "record.phase":{"$in":ACTIVE},
                })
                .session(&mut *session)
                .await?
                >= 4
            {
                return Err(AgentAccessError::Conflict.into());
            }
            fence(self, &snapshot, session).await?;
            let mut record = AgentAccessRecord {
                id,
                requester,
                proposal,
                body_sha256,
                scope_mode: mode,
                expected_scope_revision: Some(scope_revision),
                phase: AgentAccessPhase::Connecting,
                decision: Some(AgentAccessDecision::Approved {
                    command_key_hash: key_hash.clone(),
                    services: services.clone(),
                    connection: None,
                    connection_binding: None,
                    authorization: authorization.clone(),
                }),
                lease: None,
                progress: AgentAccessProgress::default(),
                approved_at: Some(now.clone()),
                retry_at: 0,
                created_at: now.clone(),
                updated_at: now.clone(),
                revision: 1,
                error: None,
            };
            super::authority::renew_current_scope(self, sandbox, &mut record, session).await?;
            let intent = document(&record)?;
            // Renewal derives authority from prior consent and runtime state.
            // A new snapshot may revalidate that state, never replace this intent.
            if original_intent
                .as_ref()
                .is_some_and(|original| original != &intent)
            {
                return Err(AgentAccessError::Conflict.into());
            }
            original_intent = Some(intent.clone());
            self.agent_access_requests
                .insert_one(intent)
                .session(session)
                .await?;
            Ok(record)
        })
    }
}

#[cfg(test)]
#[path = "owner_scope_tests.rs"]
mod tests;
