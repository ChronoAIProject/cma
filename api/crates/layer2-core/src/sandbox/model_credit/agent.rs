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
    command: AgentCommand,
}

impl FundedAgentMessage {
    pub(in crate::sandbox) fn admitted(
        command: &AdmittedAgentCommand,
    ) -> Result<Self, FundingError> {
        let effective = command
            .effective_command()
            .map_err(|_| FundingError::CheckpointChanged)?;
        Self::message(command.scope(), &command.key_sha256, effective)
    }

    fn message(scope: String, key: &str, command: AgentCommand) -> Result<Self, FundingError> {
        if !matches!(command, AgentCommand::Message { .. }) {
            return Err(FundingError::Unsupported);
        }
        let turn = format!(
            "{:x}",
            Sha256::digest(format!("cma.model-turn.v1\n{scope}\n{key}"))
        );
        Ok(Self {
            phase: PhaseId { turn, phase: 0 },
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
    ) -> Result<(), FundingError> {
        let message = FundedAgentMessage::message(
            admission.scope(),
            &admission.key_sha256,
            admission.envelope.command.clone(),
        )?;
        let selected = self.selected(agent, sandbox).await?;
        let limit = self
            .gateway
            .first_phase_limit(&selected, &message.command)
            .await?;
        self.credit()
            .reserve(
                message.phase.turn.clone(),
                &selected,
                limit,
                &message.request(None),
            )
            .await?;
        Ok(())
    }

    pub(in crate::sandbox) async fn check_message(
        &self,
        message: &FundedAgentMessage,
        agent: &Agent,
        sandbox: &Sandbox,
    ) -> Result<(), FundingError> {
        let selected = self.selected(agent, sandbox).await?;
        self.credit()
            .check_dispatch(&message.phase, &selected, &message.request(None))
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
        let checkpoint = self
            .credit()
            .dispatch(&message.phase, &selected, message.request(Some(target)))
            .await?;
        match checkpoint.state {
            PhaseState::Settled(effect) => Ok(effect),
            _ => Err(FundingError::ReconciliationRequired),
        }
    }

    /// Recover the same payer reservation and effect after an uncertain reply.
    pub async fn reconcile_message(
        &self,
        command: &AdmittedAgentCommand,
    ) -> Result<Checkpoint, FundingError> {
        let message = FundedAgentMessage::admitted(command)?;
        self.credit().reconcile(&message.phase).await
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
