use super::super::sandbox_mongo::{is_duplicate_key, is_transient, TRANSACTION_ATTEMPTS};
use super::*;
use layer2_core::resource_access::ResourceAccessStoreError;

#[async_trait::async_trait]
impl HostedChannelToolSource for MongoChannelToolStore {
    async fn resolve(
        &self,
        runtime: &RuntimeHandle,
        request: &AgentOpenRequest,
        authority_fence: u64,
    ) -> layer3_adapter::Result<Option<HostedChannelToolBinding>> {
        for _ in 0..TRANSACTION_ATTEMPTS {
            let mut transaction = self
                .resources
                .client
                .start_session()
                .await
                .map_err(|_| adapter_error(OutboxError::Unavailable))?;
            start_transaction(&mut transaction)
                .await
                .map_err(|_| adapter_error(OutboxError::Unavailable))?;
            let result = self
                .resolve_in(runtime, request, authority_fence, &mut transaction)
                .await;
            let result = match result {
                Ok(result) => result,
                Err(Error::Driver(error)) if is_transient(&error) || is_duplicate_key(&error) => {
                    let _ = transaction.abort_transaction().await;
                    continue;
                }
                Err(error) => {
                    let _ = transaction.abort_transaction().await;
                    return Err(adapter_error(error.into()));
                }
            };
            match commit_transaction(&mut transaction).await {
                CommitOutcome::Committed => return Ok(result),
                CommitOutcome::Retry(_) => continue,
                _ => return Err(adapter_error(OutboxError::Unavailable)),
            }
        }
        Err(adapter_error(OutboxError::Conflict))
    }

    async fn confirm_open(
        &self,
        binding: &HostedChannelToolBinding,
        conversation: &str,
    ) -> layer3_adapter::Result<()> {
        self.confirm(binding, conversation)
            .await
            .map_err(adapter_error)
    }
}

impl MongoChannelToolStore {
    async fn resolve_in(
        &self,
        runtime: &RuntimeHandle,
        request: &AgentOpenRequest,
        fence: u64,
        transaction: &mut ClientSession,
    ) -> Result<Option<HostedChannelToolBinding>, Error> {
        let id = layer2_core::AgentId(request.options.slot.0.clone());
        let snapshot =
            match super::super::agent_provenance::read_snapshot(&self.resources, &id, transaction)
                .await
            {
                Ok(snapshot) => snapshot,
                Err(super::super::resource_access::ReadError::Domain(
                    ResourceAccessStoreError::NotFound,
                )) => return Ok(None),
                Err(_) => return Err(OutboxError::ToolUnavailable.into()),
            };
        if !snapshot.creation.as_ref().is_some_and(|creation| {
            matches!(
                creation.creation_origin,
                AgentCreationOrigin::ChannelBridge { .. }
            )
        }) {
            return Ok(None);
        }
        let creation = snapshot
            .creation_provenance(&id)
            .map_err(|_| OutboxError::ToolUnavailable)?;
        let agent = snapshot
            .resource
            .lineage
            .agent
            .as_ref()
            .ok_or(OutboxError::ToolUnavailable)?;
        let sandbox = &snapshot.resource.lineage.sandbox;
        let tree = snapshot
            .resource
            .lineage
            .worktree
            .as_ref()
            .ok_or(OutboxError::ToolUnavailable)?;
        let expected = AgentOpenRequest::new(
            format!("agent:{}:open:v1", agent.id),
            crate::sandbox::runtime_options(agent, sandbox, tree),
            None,
        );
        let binding = sandbox
            .runtime
            .as_ref()
            .ok_or(OutboxError::ToolUnavailable)?;
        if !request.has_valid_sha256()
            || request != &expected
            || binding.runtime_id != runtime.0
            || binding.provision_generation.is_none_or(|v| v == 0)
            || !valid_fence(agent, fence)
            || agent.requested_stop_reason.is_some()
            || !agent.status.is_live()
        {
            return Err(OutboxError::ToolUnavailable.into());
        }
        let id = records::identity(&agent.id, &agent.created_at, &agent.creation_fingerprint)?;
        let previous = self
            .capabilities
            .find_one(doc! {"_id":&id})
            .session(&mut *transaction)
            .await?
            .as_ref()
            .map(records::read)
            .transpose()?;
        let record = match previous.as_ref() {
            Some(record) if record.revoked => return Err(OutboxError::ToolUnavailable.into()),
            Some(record)
                if &record.runtime == binding
                    && record.request_sha256 == request.request_sha256 =>
            {
                record.clone()
            }
            Some(record) if record.runtime.provision_generation >= binding.provision_generation => {
                return Err(OutboxError::Conflict.into())
            }
            _ => {
                let token = mint()?;
                CapabilityRecord {
                    id: id.clone(),
                    agent_id: agent.id.clone(),
                    agent_created_at: agent.created_at.clone(),
                    creation_fingerprint: creation.creation_fingerprint,
                    runtime: binding.clone(),
                    request_sha256: request.request_sha256.clone(),
                    token_sha256: sha(token.expose().as_bytes()),
                    secret: self
                        .cipher
                        .seal(COLLECTION, id.as_bytes(), "bearer", token.expose())
                        .map_err(unavailable)?,
                    opened_conversation: None,
                    revoked: false,
                    revision: previous
                        .as_ref()
                        .map_or(Some(1), |record| record.revision.checked_add(1))
                        .ok_or(OutboxError::Corrupt)?,
                }
            }
        };
        super::super::resource_access::fence_lineage(
            &self.resources,
            &snapshot.resource,
            transaction,
        )
        .await
        .map_err(|_| OutboxError::Conflict)?;
        let document = records::document(&record)?;
        if let Some(previous) = previous {
            let updated = self
                .capabilities
                .replace_one(
                    doc! {"_id":&id,"revision":previous.revision as i64,"revoked":false},
                    document,
                )
                .session(&mut *transaction)
                .await?;
            if updated.matched_count != 1 {
                return Err(OutboxError::Conflict.into());
            }
        } else {
            self.capabilities
                .insert_one(document)
                .session(&mut *transaction)
                .await?;
        }
        let token = self
            .cipher
            .open(COLLECTION, id.as_bytes(), "bearer", record.secret.clone())
            .map_err(unavailable)?;
        if sha(token.as_bytes()) != record.token_sha256 {
            return Err(OutboxError::Corrupt.into());
        }
        Ok(Some(
            HostedChannelToolBinding::new(
                &self.public_origin,
                RuntimeCredential::new(token),
                runtime.clone(),
                request,
                fence,
                record.opened_conversation,
            )
            .map_err(|_| OutboxError::ToolUnavailable)?,
        ))
    }

    async fn confirm(
        &self,
        binding: &HostedChannelToolBinding,
        conversation: &str,
    ) -> Result<(), OutboxError> {
        let (runtime, slot, request_sha, fence) = binding.scope();
        if conversation.is_empty()
            || conversation.len() > 512
            || conversation.chars().any(char::is_control)
        {
            return Err(OutboxError::Bounds);
        }
        let mut transaction = self
            .resources
            .client
            .start_session()
            .await
            .map_err(unavailable)?;
        start_transaction(&mut transaction)
            .await
            .map_err(unavailable)?;
        let result = self
            .confirm_in(
                runtime,
                slot,
                request_sha,
                fence,
                conversation,
                &mut transaction,
            )
            .await;
        if let Err(error) = result {
            let _ = transaction.abort_transaction().await;
            return Err(error);
        }
        match commit_transaction(&mut transaction).await {
            CommitOutcome::Committed => Ok(()),
            _ => Err(OutboxError::Unavailable),
        }
    }

    async fn confirm_in(
        &self,
        runtime: &RuntimeHandle,
        slot: &layer3_adapter::AgentSlot,
        request_sha: &str,
        fence: u64,
        conversation: &str,
        transaction: &mut ClientSession,
    ) -> Result<(), OutboxError> {
        let snapshot = super::super::agent_provenance::read_snapshot(
            &self.resources,
            &layer2_core::AgentId(slot.0.clone()),
            transaction,
        )
        .await
        .map_err(|_| OutboxError::ToolUnavailable)?;
        let agent = snapshot
            .resource
            .lineage
            .agent
            .as_ref()
            .ok_or(OutboxError::ToolUnavailable)?;
        let creation = snapshot
            .creation_provenance(&agent.id)
            .map_err(|_| OutboxError::ToolUnavailable)?;
        if !matches!(
            creation.creation_origin,
            AgentCreationOrigin::ChannelBridge { .. }
        ) || !valid_fence(agent, fence)
            || agent.requested_stop_reason.is_some()
            || !agent.status.is_live()
        {
            return Err(OutboxError::ToolUnavailable);
        }
        let id = records::identity(&agent.id, &agent.created_at, &agent.creation_fingerprint)?;
        let document = self
            .capabilities
            .find_one(doc! {"_id":&id,"revoked":false})
            .session(&mut *transaction)
            .await
            .map_err(unavailable)?
            .ok_or(OutboxError::ToolUnavailable)?;
        let mut record = records::read(&document)?;
        if record.runtime.runtime_id != runtime.0
            || snapshot.resource.lineage.sandbox.runtime.as_ref() != Some(&record.runtime)
            || record.request_sha256 != request_sha
            || record
                .opened_conversation
                .as_deref()
                .is_some_and(|id| id != conversation)
            || agent
                .conversation
                .conversation_id
                .as_deref()
                .is_some_and(|id| id != conversation)
        {
            return Err(OutboxError::SourceMismatch);
        }
        let revision = record.revision;
        record.revision = revision.checked_add(1).ok_or(OutboxError::Corrupt)?;
        record.opened_conversation = Some(conversation.into());
        super::super::resource_access::fence_lineage(
            &self.resources,
            &snapshot.resource,
            transaction,
        )
        .await
        .map_err(|_| OutboxError::Conflict)?;
        let updated = self
            .capabilities
            .replace_one(
                doc! {"_id":&id,"revision":revision as i64,"revoked":false},
                records::document(&record)?,
            )
            .session(&mut *transaction)
            .await
            .map_err(unavailable)?;
        if updated.matched_count != 1 {
            return Err(OutboxError::Conflict);
        }
        Ok(())
    }
}

pub(super) fn valid_fence(agent: &layer2_core::sandbox::Agent, fence: u64) -> bool {
    fence > 0
        && agent
            .work
            .lease
            .as_ref()
            .map_or(agent.work.next_fence == fence, |lease| {
                lease.fence == fence
                    && layer2_core::seconds_from_rfc3339(&lease.expires_at).is_some_and(|expiry| {
                        std::time::UNIX_EPOCH + Duration::from_secs(expiry)
                            > std::time::SystemTime::now()
                    })
            })
}

fn mint() -> Result<RuntimeCredential, OutboxError> {
    use ring::rand::{SecureRandom, SystemRandom};
    let mut bytes = [0_u8; 32];
    SystemRandom::new().fill(&mut bytes).map_err(unavailable)?;
    let mut value = String::from("cma_ct_");
    for byte in bytes {
        use std::fmt::Write;
        write!(&mut value, "{byte:02x}").map_err(unavailable)?;
    }
    Ok(RuntimeCredential::new(value))
}

enum Error {
    Core(OutboxError),
    Driver(mongodb::error::Error),
}
impl From<OutboxError> for Error {
    fn from(value: OutboxError) -> Self {
        Self::Core(value)
    }
}
impl From<mongodb::error::Error> for Error {
    fn from(value: mongodb::error::Error) -> Self {
        Self::Driver(value)
    }
}
impl From<Error> for OutboxError {
    fn from(value: Error) -> Self {
        match value {
            Error::Core(error) => error,
            Error::Driver(_) => Self::Unavailable,
        }
    }
}
