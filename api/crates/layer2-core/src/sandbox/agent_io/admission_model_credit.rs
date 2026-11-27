use super::*;
use crate::sandbox::{
    model_credit::{AdmissionHold, AgentTurnFunding, Checkpoint, FundingError},
    task_experience::TaskExperiencePolicy,
    AgentApprovalDecision,
};

pub(super) struct ModelCreditRefusal {
    funding: Option<Arc<AgentTurnFunding>>,
    pending: Option<AdmissionHold>,
}

impl ModelCreditRefusal {
    pub(super) async fn settle(&mut self) -> Result<(), CommandAdmissionError> {
        if let Some(held) = &self.pending {
            let funding = self.funding.as_ref().ok_or(CommandLedgerError::Corrupt)?;
            report_refusal(funding, held).await?;
            self.pending = None;
        }
        Ok(())
    }
}

impl Drop for ModelCreditRefusal {
    fn drop(&mut self) {
        let (Some(funding), Some(held)) = (self.funding.take(), self.pending.take()) else {
            return;
        };
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                if report_refusal(&funding, &held).await.is_err() {
                    tracing::warn!(
                        "Could not settle a definitively refused model credit participant"
                    );
                }
            });
        }
    }
}

impl AgentCommandService {
    pub fn with_task_experience(self: Arc<Self>, policy: Arc<TaskExperiencePolicy>) -> Arc<Self> {
        Arc::new(Self {
            task_experience: policy,
            ..(*self).clone()
        })
    }

    pub(super) fn model_credit_refusal(&self) -> ModelCreditRefusal {
        ModelCreditRefusal {
            funding: self.model_funding.clone(),
            pending: None,
        }
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
        may_refresh: bool,
        deferred_refusal: &mut ModelCreditRefusal,
    ) -> Result<Result<CommandReservationResult, CommandLedgerError>, CommandAdmissionError> {
        let capability = command_capability(&request.envelope.command);
        let funding = required
            .then(|| {
                self.model_funding
                    .as_ref()
                    .ok_or(AgentOperationError::ModelFundingUnsupported)
            })
            .transpose()?;
        let mut refusal = self.model_credit_refusal();
        if let Some(funding) = funding {
            self.reserve_model_credit(funding, authorized, &request, &mut refusal)
                .await?;
            let held = refusal
                .pending
                .as_ref()
                .ok_or(CommandLedgerError::Corrupt)?;
            if let Some(previous) = deferred_refusal.pending.as_ref() {
                // Register the successor before retiring the known-aborted
                // participant so the same reservation stays funded throughout.
                if held.checkpoint != previous.checkpoint {
                    refusal.settle().await?;
                    return Err(CommandLedgerError::Conflict.into());
                }
                if let Err(error) = deferred_refusal.settle().await {
                    refusal.settle().await?;
                    return Err(error);
                }
            }
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
                refusal.settle().await?;
                return Err(error);
            }
        }
        let result = self
            .with_authority_bound(authorized, capability, async {
                // During the store call a dropped future may hide a commit.
                let in_flight = refusal.pending.take();
                let result = self.store.reserve_command(request).await;
                refusal.pending =
                    in_flight.filter(|held| admission_refused(&result, &held.checkpoint));
                result
            })
            .await;
        if may_refresh && matches!(&result, Ok(Err(CommandLedgerError::KnownAborted))) {
            deferred_refusal.pending = refusal.pending.take();
        } else {
            refusal.settle().await?;
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
        refusal: &mut ModelCreditRefusal,
    ) -> Result<(), CommandAdmissionError> {
        let lineage = &authorized.snapshot.lineage;
        let agent = lineage.agent.as_ref().ok_or(CommandLedgerError::Corrupt)?;
        self.with_authority_bound(
            authorized,
            command_capability(&request.envelope.command),
            async {
                // Keep confirmed custody even if the outer authorization fence
                // expires as this funding request returns.
                refusal.pending = Some(
                    funding
                        .reserve_admission(request, agent, &lineage.sandbox)
                        .await?,
                );
                Ok::<(), FundingError>(())
            },
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

async fn report_refusal(
    funding: &AgentTurnFunding,
    held: &AdmissionHold,
) -> Result<(), CommandAdmissionError> {
    bounded(funding.refuse_admission(held))
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
        | Err(
            CommandLedgerError::Corrupt
            | CommandLedgerError::Unavailable
            | CommandLedgerError::ControlBusy,
        ) => false,
        Err(
            CommandLedgerError::Budget(_)
            | CommandLedgerError::AdmissionPaused
            | CommandLedgerError::SessionCapacityUnavailable
            | CommandLedgerError::Invalid
            | CommandLedgerError::NotFound
            | CommandLedgerError::IdempotencyConflict
            | CommandLedgerError::Conflict
            | CommandLedgerError::KnownAborted
            | CommandLedgerError::StaleClaim
            | CommandLedgerError::NotAdmissible
            | CommandLedgerError::QueueFull,
        ) => true,
    }
}
