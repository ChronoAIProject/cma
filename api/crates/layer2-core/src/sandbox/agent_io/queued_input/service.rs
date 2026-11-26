use super::super::admission_service::IO_BOUND;
use super::super::{
    AgentCommand, AgentCommandService, CommandAdmissionError, CommandLedgerError,
    CommandSubmission, CommandSubmissionSource, QueuedCancellationReservation,
};
use super::{
    QueuedInputOutcome, QueuedInputReservation, QueuedInputStatusView, QueuedInputSubmission,
    QueuedInputView,
};
use crate::resource_access::{AccessPurpose, CallerContext, ResourceCapability, ResourceTarget};

const QUEUED_EDIT_ATTEMPTS: usize = 3;

impl AgentCommandService {
    pub async fn edit_queued_input(
        &self,
        context: &CallerContext,
        credential: &str,
        source: CommandSubmissionSource,
        submission: QueuedInputSubmission,
    ) -> Result<QueuedInputStatusView, super::super::QueuedCancellationError> {
        super::queued_input_fingerprint(
            &submission.agent_id,
            &submission.target,
            &submission.text,
        )?;
        let deadline = tokio::time::Instant::now() + IO_BOUND;
        let edit = CommandSubmission {
            agent_id: submission.agent_id,
            command: AgentCommand::Interrupt,
            idempotency_key: submission.idempotency_key,
            expected_agent_revision: None,
            requested_model: None,
        };
        let (mut authorized, mut command) = tokio::time::timeout_at(
            deadline,
            self.prepare(context, credential, source, edit.clone()),
        )
        .await
        .map_err(|_| CommandLedgerError::Unavailable)??;
        let mut attempt_deadline = command
            .authority_deadline
            .map_or(deadline, |limit| limit.min(deadline));
        command.authority_deadline = Some(attempt_deadline);
        let resource = ResourceTarget::Agent {
            agent_id: edit.agent_id.clone(),
        };
        let capability = ResourceCapability::OperateAgent;
        for attempt in 1..=QUEUED_EDIT_ATTEMPTS {
            let agent = authorized
                .snapshot
                .lineage
                .agent
                .as_ref()
                .ok_or(CommandLedgerError::Corrupt)?;
            let request = QueuedInputReservation {
                text: submission.text.clone(),
                cancellation: QueuedCancellationReservation {
                    target: submission.target.clone(),
                    observed_agent_revision: agent.revision,
                    agent_created_at: agent.created_at.clone(),
                    command: command.clone(),
                },
            };
            request.validate()?;
            let result = tokio::time::timeout_at(
                attempt_deadline,
                self.with_authority_bound(
                    &authorized,
                    capability,
                    self.store.edit_queued_input(request.clone()),
                ),
            )
            .await
            .map_err(|_| CommandLedgerError::Unavailable)?;
            tokio::time::timeout_at(
                attempt_deadline,
                self.fence(&authorized, &resource, capability),
            )
            .await
            .map_err(|_| CommandLedgerError::Unavailable)??;
            let outcome = result?.inspect_err(|error| {
                if matches!(
                    error,
                    CommandLedgerError::Conflict
                        | CommandLedgerError::StaleClaim
                        | CommandLedgerError::NotAdmissible
                ) {
                    tracing::info!(attempt, reason = "store_conflict", "Queued edit refused");
                }
            })?;
            match outcome {
                QueuedInputOutcome::NotAdmitted => {
                    return Err(super::super::QueuedCancellationError::NotAdmitted);
                }
                QueuedInputOutcome::Applied(value) => {
                    let value =
                        request.replay(value.record, Some(value.binding), value.response)?;
                    return Ok(value.into());
                }
                QueuedInputOutcome::StaleObservation => {
                    if attempt == QUEUED_EDIT_ATTEMPTS {
                        tracing::info!(
                            attempt,
                            reason = "stale_observation_exhausted",
                            "Queued edit refused"
                        );
                        break;
                    }
                    tracing::info!(
                        attempt,
                        reason = "stale_observation",
                        "Refreshing queued edit authorization"
                    );
                }
            }

            // This store outcome proves the transaction stopped before writing.
            // Keep the original edit and refresh only the live observation.
            command.require_current_authority()?;
            let (fresh_authorized, fresh) = tokio::time::timeout_at(
                attempt_deadline,
                self.prepare(context, credential, source, edit.clone()),
            )
            .await
            .map_err(|_| CommandLedgerError::Unavailable)??;
            let lineage = &authorized.snapshot.lineage;
            let fresh_lineage = &fresh_authorized.snapshot.lineage;
            let current = fresh_lineage
                .agent
                .as_ref()
                .ok_or(CommandLedgerError::Corrupt)?;
            if current.created_at != agent.created_at
                || current.workspace_id != agent.workspace_id
                || current.worktree_id != agent.worktree_id
                || fresh_lineage.workspace.as_ref().map(|row| &row.created_at)
                    != lineage.workspace.as_ref().map(|row| &row.created_at)
                || current.conversation.runtime_generation != agent.conversation.runtime_generation
                || fresh.parent.runtime != command.parent.runtime
                || fresh.policy != command.policy
                || fresh.binding != command.binding
                || fresh.envelope.actor != command.envelope.actor
                || fresh.envelope.owner != command.envelope.owner
                || fresh.envelope.authorization != command.envelope.authorization
                || fresh_authorized.decision.share_revisions != authorized.decision.share_revisions
                || !fresh_authorized
                    .decision
                    .capabilities()
                    .eq(authorized.decision.capabilities())
            {
                tracing::info!(
                    attempt,
                    reason = "refresh_scope_changed",
                    "Queued edit refused"
                );
                return Err(CommandLedgerError::Conflict.into());
            }
            command.expected_agent_revision = fresh.expected_agent_revision;
            command.parent = fresh.parent;
            command.admission_audit = fresh.admission_audit;
            attempt_deadline = fresh
                .authority_deadline
                .map_or(attempt_deadline, |limit| limit.min(attempt_deadline));
            command.authority_deadline = Some(attempt_deadline);
            authorized = fresh_authorized;
        }
        Err(CommandLedgerError::Conflict.into())
    }
}

impl AgentCommandService {
    pub async fn list_queued_inputs(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        agent_id: &crate::AgentId,
    ) -> Result<Vec<QueuedInputView>, CommandAdmissionError> {
        if !context.validate_principal() || context.actor.ordinary_subject().is_none() {
            return Err(CommandLedgerError::NotFound.into());
        }
        let target = ResourceTarget::Agent {
            agent_id: agent_id.clone(),
        };
        let capability = ResourceCapability::OperateAgent;
        let authorized = super::super::admission_service::bounded(self.access.authorize(
            context,
            credential,
            &target,
            capability,
            AccessPurpose::Read,
        ))
        .await??;
        let result = self
            .with_authority_bound(
                &authorized,
                capability,
                self.store.list_queued_inputs(agent_id),
            )
            .await;
        self.fence(&authorized, &target, capability).await?;
        let agent = authorized
            .snapshot
            .lineage
            .agent
            .as_ref()
            .ok_or(CommandLedgerError::Corrupt)?;
        let owner = authorized
            .snapshot
            .scope(&target)
            .map_err(crate::resource_access::ResourceAccessError::from)?
            .owner;
        let records = result??;
        let mut values = Vec::new();
        for record in records {
            let response_id = record
                .receipt
                .response_id
                .as_ref()
                .ok_or(CommandLedgerError::Corrupt)?;
            super::super::queued_cancellation::reservation::validate_source(
                agent,
                &owner,
                &record,
                response_id,
            )?;
            values.push(QueuedInputView::from_record(&record)?);
        }
        Ok(values)
    }
}
