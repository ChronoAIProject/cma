use super::*;
use crate::sandbox::{
    model_credit::{AdmissionHold, AgentTurnFunding, Checkpoint},
    task_experience::TaskExperiencePolicy,
    AgentApprovalDecision,
};

impl AgentCommandService {
    pub fn with_task_experience(self: Arc<Self>, policy: Arc<TaskExperiencePolicy>) -> Arc<Self> {
        Arc::new(Self {
            task_experience: policy,
            ..(*self).clone()
        })
    }

    pub fn with_model_credit_required(self: Arc<Self>, required: bool) -> Arc<Self> {
        Arc::new(Self {
            model_credit_required: required,
            ..(*self).clone()
        })
    }

    pub fn with_model_funding(
        self: Arc<Self>,
        funding: Option<Arc<AgentTurnFunding>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            model_funding: funding,
            ..(*self).clone()
        })
    }

    pub(super) async fn reserve_with_model_credit(
        &self,
        authorized: &AuthorizedResource,
        target: &ResourceTarget,
        mut request: CommandReservation,
        required: bool,
    ) -> Result<Result<CommandReservationResult, CommandLedgerError>, CommandAdmissionError> {
        let capability = command_capability(&request.envelope.command);
        let funding = required
            .then(|| {
                self.model_funding
                    .as_ref()
                    .ok_or(AgentOperationError::ModelFundingUnsupported)
            })
            .transpose()?;
        let hold = if let Some(funding) = funding {
            let held = self
                .reserve_model_credit(funding, authorized, &request)
                .await?;
            let identity = held
                .checkpoint
                .reservation
                .command
                .as_ref()
                .ok_or(CommandLedgerError::Corrupt)?;
            let command_ref = identity.command_id.to_string();
            request.envelope.command_id = identity.command_id.clone();
            request.envelope.source.reference = command_ref.clone();
            request.envelope.correlation_id = command_ref;
            request.response_id = Some(identity.response_id.clone());
            if let Err(error) = self.fence(authorized, target, capability).await {
                if held.exclusive {
                    release_refused(funding, &held.checkpoint).await?;
                }
                return Err(error);
            }
            held.exclusive.then_some((funding, held.checkpoint))
        } else {
            None
        };
        let mut definitively_refused = hold.is_some();
        let result = self
            .with_authority_bound(authorized, capability, async {
                // Cancellation after this point may leave a durable receipt.
                definitively_refused = false;
                let result = self.store.reserve_command(request).await;
                definitively_refused = hold
                    .as_ref()
                    .is_some_and(|(_, held)| admission_refused(&result, held));
                result
            })
            .await;
        if let Some((funding, held)) = hold.filter(|_| definitively_refused) {
            release_refused(funding, &held).await?;
        }
        // An unavailable write or a failed authorization response may hide a commit.
        // Keep the exact hold so a worker can settle it from the durable receipt.
        self.fence(authorized, target, capability).await?;
        result
    }

    async fn reserve_model_credit(
        &self,
        funding: &AgentTurnFunding,
        authorized: &AuthorizedResource,
        request: &CommandReservation,
    ) -> Result<AdmissionHold, CommandAdmissionError> {
        let lineage = &authorized.snapshot.lineage;
        let agent = lineage.agent.as_ref().ok_or(CommandLedgerError::Corrupt)?;
        self.with_authority_bound(
            authorized,
            command_capability(&request.envelope.command),
            funding.reserve_admission(request, agent, &lineage.sandbox),
        )
        .await?
        .map_err(|error| AgentOperationError::from(error).into())
    }

    pub(in crate::sandbox::agent_io) fn require_model_credit(
        &self,
        command: &AgentCommand,
    ) -> Result<(), AgentOperationError> {
        let potentially_billable = matches!(
            command,
            AgentCommand::Message { .. }
                | AgentCommand::Steer { .. }
                | AgentCommand::AnswerApproval {
                    decision: AgentApprovalDecision::Allow | AgentApprovalDecision::Deny,
                    ..
                }
        );
        crate::sandbox::model_credit::require_available(
            self.model_credit_required && potentially_billable,
        )
    }
}

async fn release_refused(
    funding: &AgentTurnFunding,
    held: &Checkpoint,
) -> Result<(), CommandAdmissionError> {
    bounded(funding.release_refused(held))
        .await?
        .map_err(|error| AgentOperationError::from(error).into())
}

fn admission_refused(
    result: &Result<CommandReservationResult, CommandLedgerError>,
    held: &Checkpoint,
) -> bool {
    match result {
        Ok(CommandReservationResult::Existing(record)) => {
            !held.reservation.command.as_ref().is_some_and(|identity| {
                identity.command_id == record.receipt.envelope.command_id
                    && Some(&identity.response_id) == record.receipt.response_id.as_ref()
            })
        }
        Ok(CommandReservationResult::Created(_))
        | Err(CommandLedgerError::Corrupt | CommandLedgerError::Unavailable) => false,
        Err(
            CommandLedgerError::Budget(_)
            | CommandLedgerError::AdmissionPaused
            | CommandLedgerError::SessionCapacityUnavailable
            | CommandLedgerError::Invalid
            | CommandLedgerError::NotFound
            | CommandLedgerError::IdempotencyConflict
            | CommandLedgerError::Conflict
            | CommandLedgerError::StaleClaim
            | CommandLedgerError::NotAdmissible
            | CommandLedgerError::QueueFull,
        ) => true,
    }
}
