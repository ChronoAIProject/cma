use super::*;
use crate::sandbox::{
    agent_io::{AdmittedAgentCommand, AgentCommand, CommandReservation},
    Agent, AgentRuntimeTarget, Sandbox,
};
use sha2::{Digest, Sha256};
use std::sync::Arc;

/// The durable command determines the request identity. The exact runtime
/// target is resolved and fenced by CMA immediately before dispatch.
pub struct ModelPhaseRequest {
    pub command: AgentCommand,
    pub target: Option<AgentRuntimeTarget>,
}

#[async_trait]
pub trait AgentModelGateway: FundedModelGateway<Request = ModelPhaseRequest> {
    /// Return facts from the credential selection used by this gateway.
    async fn selected_credential(
        &self,
        agent: &Agent,
        sandbox: &Sandbox,
    ) -> Result<CredentialBinding, FundingError>;

    async fn first_phase_limit(
        &self,
        selected: &CredentialBinding,
        request: &AgentCommand,
    ) -> Result<NonZeroU64, FundingError>;
}

/// Shared by command admission and the runtime dispatch service. A supported
/// gateway must actually perform its model request through the funded boundary.
pub struct AgentTurnFunding {
    authority: Arc<dyn ReservationAuthority>,
    gateway: Arc<dyn AgentModelGateway>,
}

#[derive(Clone)]
pub(in crate::sandbox) struct FundedAgentMessage {
    phase: PhaseId,
    command_id: AgentCommandId,
    command: AgentCommand,
}

impl FundedAgentMessage {
    pub(in crate::sandbox) fn admitted(
        command: &AdmittedAgentCommand,
    ) -> Result<Self, FundingError> {
        let effective = command
            .effective_command()
            .map_err(|_| FundingError::CheckpointChanged)?;
        Self::message(
            command.scope(),
            &command.key_sha256,
            command.receipt.envelope.command_id.clone(),
            effective,
        )
    }

    fn message(
        scope: String,
        key: &str,
        command_id: AgentCommandId,
        command: AgentCommand,
    ) -> Result<Self, FundingError> {
        if !matches!(command, AgentCommand::Message { .. }) {
            return Err(FundingError::Unsupported);
        }
        let turn = format!(
            "{:x}",
            Sha256::digest(format!("cma.model-turn.v1\n{scope}\n{key}"))
        );
        Ok(Self {
            phase: PhaseId { turn, phase: 0 },
            command_id,
            command,
        })
    }

    fn request(&self, target: Option<AgentRuntimeTarget>) -> ModelPhaseRequest {
        ModelPhaseRequest {
            command: self.command.clone(),
            target,
        }
    }
}

impl AgentTurnFunding {
    pub fn new(
        authority: Arc<dyn ReservationAuthority>,
        gateway: Arc<dyn AgentModelGateway>,
    ) -> Arc<Self> {
        Arc::new(Self { authority, gateway })
    }

    fn credit(&self) -> ModelCredit<'_, dyn ReservationAuthority, dyn AgentModelGateway> {
        ModelCredit::new(self.authority.as_ref(), self.gateway.as_ref())
    }

    pub(in crate::sandbox) async fn reserve_admission(
        &self,
        admission: &CommandReservation,
        agent: &Agent,
        sandbox: &Sandbox,
    ) -> Result<Checkpoint, FundingError> {
        let message = FundedAgentMessage::message(
            admission.scope(),
            &admission.key_sha256,
            admission.envelope.command_id.clone(),
            admission.envelope.command.clone(),
        )?;
        let selected = self.selected(agent, sandbox).await?;
        let limit = self
            .gateway
            .first_phase_limit(&selected, &message.command)
            .await?;
        let held = self
            .credit()
            .reserve_first(
                message.phase.clone(),
                Some(message.command_id.clone()),
                &selected,
                limit,
                &message.request(None),
            )
            .await?;
        if held.state != PhaseState::Reserved {
            return Err(FundingError::CheckpointChanged);
        }
        Ok(held)
    }

    pub(in crate::sandbox) async fn release_refused(
        &self,
        held: &Checkpoint,
    ) -> Result<(), FundingError> {
        self.authority.release_refused(held).await
    }

    pub(in crate::sandbox) async fn pending_commands(
        &self,
        page: &PageRequest,
    ) -> Result<Page<AgentCommandId>, FundingError> {
        self.authority.pending_commands(page).await
    }

    pub(in crate::sandbox) async fn check_message(
        &self,
        message: &FundedAgentMessage,
        agent: &Agent,
        sandbox: &Sandbox,
    ) -> Result<(), FundingError> {
        let selected = self.selected(agent, sandbox).await?;
        self.credit()
            .check_dispatch(
                &message.phase,
                Some(&message.command_id),
                &selected,
                &message.request(None),
            )
            .await?;
        Ok(())
    }

    pub(in crate::sandbox) async fn dispatch_message(
        &self,
        message: &FundedAgentMessage,
        agent: &Agent,
        sandbox: &Sandbox,
        target: AgentRuntimeTarget,
    ) -> Result<Effect, FundingError> {
        let selected = self.selected(agent, sandbox).await?;
        let credit = self.credit();
        let request = message.request(Some(target));
        let held = credit
            .check_dispatch(
                &message.phase,
                Some(&message.command_id),
                &selected,
                &request,
            )
            .await?;
        let checkpoint = credit.dispatch_held(held, request).await?;
        match checkpoint.state {
            PhaseState::Settled(effect) => Ok(effect),
            _ => Err(FundingError::ReconciliationRequired),
        }
    }

    /// Reconcile against the exact durable command and original payer. A
    /// terminal row lets us fence and settle a hold that never began. A gateway
    /// observation is required once a dispatch has begun, even after cancellation.
    pub(in crate::sandbox) async fn reconcile_message(
        &self,
        command: &AdmittedAgentCommand,
    ) -> Result<(), FundingError> {
        if !matches!(
            command.receipt.envelope.command,
            AgentCommand::Message { .. }
        ) {
            return Ok(());
        }
        let message = FundedAgentMessage::message(
            command.scope(),
            &command.key_sha256,
            command.receipt.envelope.command_id.clone(),
            command.receipt.envelope.command.clone(),
        )?;
        let held = match self.authority.read(&message.phase).await {
            Err(FundingError::NoReservation) => return Ok(()),
            result => result?,
        };
        if held.reservation.id != message.phase
            || held.reservation.command_id.as_ref() != Some(&message.command_id)
            || held.reservation.request_digest != self.gateway.digest(&message.request(None))
        {
            return Err(FundingError::CheckpointChanged);
        }
        let credit = self.credit();
        match held.state {
            PhaseState::InFlight => credit.reconcile_held(&held).await.map(|_| ()),
            PhaseState::Reserved if command.status.is_terminal() => {
                credit.settle(&held, Effect::NotStarted).await.map(|_| ())
            }
            _ => Ok(()),
        }
    }

    async fn selected(
        &self,
        agent: &Agent,
        sandbox: &Sandbox,
    ) -> Result<CredentialBinding, FundingError> {
        use crate::profiles::ModelAccess;
        let expected_source = match sandbox.profile.content.model_access {
            ModelAccess::ChronoaiManaged => CredentialSource::Managed,
            ModelAccess::OwnCredentials => CredentialSource::Own,
        };
        let selected = self.gateway.selected_credential(agent, sandbox).await?;
        if selected.source != expected_source {
            return Err(FundingError::BindingChanged);
        }
        Ok(selected)
    }
}
