use super::*;
use layer2_core::{sandbox::AgentRuntimeTarget, RuntimeCapabilities};

pub(crate) struct ChannelControlContextObservation {
    pub reference: String,
    pub target: AgentRuntimeTarget,
    pub capabilities: RuntimeCapabilities,
}

impl MongoBridgeReplyToolStore {
    #[cfg(test)]
    pub(crate) async fn control_context(
        &self,
        capability: &ChannelReplySessionAuthorization,
    ) -> Result<Option<String>, OutboxError> {
        Ok(self
            .control_context_observation(capability)
            .await?
            .map(|context| context.reference))
    }

    pub(crate) async fn control_context_observation(
        &self,
        capability: &ChannelReplySessionAuthorization,
    ) -> Result<Option<ChannelControlContextObservation>, OutboxError> {
        let scope = match self.read_current(capability, None).await {
            Ok(scope) => scope,
            Err(OutboxError::ToolUnavailable) => return Ok(None),
            Err(error) => return Err(error),
        };
        let validation = self
            .validator
            .validate_connection(scope.source().owner(), scope.source().config())
            .await
            .map_err(|_| OutboxError::AuthorityUnavailable)?;
        let reference = format!("ccr_{}", uuid::Uuid::new_v4().simple());
        // Freeze the first observed source/turn/consent even after a known
        // aborted transaction. An unknown commit is recovered by its mapping.
        let mut candidate = None;
        for attempt in 0..TRANSACTION_ATTEMPTS {
            if attempt > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(10 * (1 << attempt))).await;
            }
            scope.require_current()?;
            let mut session = self
                .invokes
                .bridges
                .client
                .start_session()
                .await
                .map_err(|_| OutboxError::Unavailable)?;
            start_transaction(&mut session)
                .await
                .map_err(|_| OutboxError::Unavailable)?;
            let result = self
                .control_reference_transaction(
                    &scope,
                    &validation,
                    &reference,
                    &mut candidate,
                    &mut session,
                )
                .await;
            let result = match result {
                Ok(value) => value,
                Err(error) => {
                    let _ = session.abort_transaction().await;
                    if error.retryable() {
                        continue;
                    }
                    return Err(error.into_core());
                }
            };
            if let Err(error) = scope.require_current() {
                let _ = session.abort_transaction().await;
                return Err(error);
            }
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(result),
                CommitOutcome::Retry(_) => continue,
                CommitOutcome::Unknown(_) | CommitOutcome::Failed(_) => {
                    return Err(OutboxError::Unavailable)
                }
            }
        }
        Err(OutboxError::Unavailable)
    }

    async fn control_reference_transaction(
        &self,
        scope: &ChannelReplyTurnAuthorization,
        validation: &BridgeValidation,
        reference: &str,
        candidate: &mut Option<ChannelControlReferenceRecord>,
        session: &mut ClientSession,
    ) -> Result<Option<ChannelControlContextObservation>, MutationError> {
        self.fence_turn(scope, validation, session).await?;
        let snapshot = self
            .turn_snapshot(
                &scope.source().target().agent_id,
                scope.source().command_id().as_str(),
                session,
            )
            .await?;
        let grant = self
            .invokes
            .grants
            .find_one(doc! {"_id":snapshot.source_grant_id()?})
            .session(&mut *session)
            .await?
            .as_ref()
            .map(super::super::read_grant)
            .transpose()
            .map_err(|_| OutboxError::AuthorityUnavailable)?
            .ok_or(OutboxError::AuthorityUnavailable)?;
        let Some((consent, receipt)) = self
            .invokes
            .reference_consent(&grant, scope.deadline(), session)
            .await
            .map_err(|error| match error {
                super::super::MutationError::Mongo(error) => MutationError::Driver(error),
                super::super::MutationError::Invoke(_) => {
                    MutationError::Core(OutboxError::AuthorityUnavailable)
                }
            })?
        else {
            return Ok(None);
        };
        let authorization =
            snapshot.control_reference_authorization(scope, &grant, &consent, &receipt)?;
        // Take routing facts from the same fenced, authorized turn snapshot as
        // the reference. A reference supplied by a caller never selects a runtime.
        let turn = snapshot.observation(scope.capability_incarnation(), scope.deadline())?;
        let target = AgentRuntimeTarget {
            agent_id: turn.agent.id.clone(),
            runtime_generation: scope.runtime_generation(),
            conversation_id: turn
                .agent
                .conversation
                .conversation_id
                .clone()
                .ok_or(OutboxError::ToolUnavailable)?,
        };
        let capabilities = turn.agent.capabilities.clone();
        if let Some(record) = candidate.as_ref() {
            authorization.require_record(record)?;
        } else {
            *candidate = Some(authorization.retain(reference.into())?);
        }
        let retained = candidate.as_ref().ok_or(OutboxError::Corrupt)?;
        if let Some(document) = self
            .control_references
            .find_one(doc! {"lookup_sha256":authorization.lookup_sha256()?})
            .session(&mut *session)
            .await?
        {
            let existing = read_reference(&document)?;
            authorization.require_record(&existing)?;
            return Ok(Some(ChannelControlContextObservation {
                reference: existing.reference,
                target,
                capabilities,
            }));
        }
        self.control_references
            .insert_one(reference_document(retained)?)
            .session(&mut *session)
            .await?;
        Ok(Some(ChannelControlContextObservation {
            reference: retained.reference.clone(),
            target,
            capabilities,
        }))
    }
}

fn reference_document(record: &ChannelControlReferenceRecord) -> Result<Document, OutboxError> {
    record.validate()?;
    Ok(
        doc! {"_id":&record.reference,"lookup_sha256":&record.lookup_sha256,
        "document_schema_version":DOCUMENT_SCHEMA_VERSION,"record":record_bson(record).map_err(|_| OutboxError::Corrupt)?},
    )
}
pub(in crate::store::bridge_invokes) fn read_reference(
    document: &Document,
) -> Result<ChannelControlReferenceRecord, OutboxError> {
    let record: ChannelControlReferenceRecord =
        read_record(document).map_err(|_| OutboxError::Corrupt)?;
    let mut canonical = document.clone();
    if let Some(value) = canonical.remove("control_admission_fence") {
        if !matches!(value, mongodb::bson::Bson::Int64(count) if count >= 0) {
            return Err(OutboxError::Corrupt);
        }
    }
    if canonical != reference_document(&record)? {
        return Err(OutboxError::Corrupt);
    }
    Ok(record)
}
