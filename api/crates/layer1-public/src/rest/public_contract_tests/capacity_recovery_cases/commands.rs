use layer2_core::{
    sandbox::{
        agent_capacity_recovery::{
            commands::{CapacityRecoveryCommands, RecoveryStatus},
            ports::{CommitOutcome, RecoveryError},
            preflight::CapacityRecoveryPreflightService,
            state::{RecoveryPhase, RecoveryRequest, RecoveryTarget, RecoveryWork},
            CapacityRecoveryDisposition,
        },
        AgentWorkReceipt, AgentWorkStep, RuntimeBinding,
    },
    AgentId, Caller,
};
use std::sync::Arc;
pub(super) struct Commands {
    pub preflight: Arc<CapacityRecoveryPreflightService>,
    pub mode: String,
}
impl Commands {
    async fn authorize(&self, caller: &Caller, id: &AgentId) -> Result<(), RecoveryError> {
        self.preflight
            .read(caller, id)
            .await
            .map_err(|_| RecoveryError::Unauthorized)?;
        match self.mode.as_str() {
            "unavailable" => Err(RecoveryError::Unavailable),
            "conflict" => Err(RecoveryError::Conflict),
            _ => Ok(()),
        }
    }
    fn outcome(
        &self,
        request: &RecoveryRequest,
    ) -> Result<CommitOutcome<RecoveryWork>, RecoveryError> {
        match self.mode.as_str() {
            "unknown" => Ok(CommitOutcome::Unknown),
            "known_abort" => Ok(CommitOutcome::KnownAbort),
            _ => Ok(CommitOutcome::Recorded(completed(request))),
        }
    }
}
#[async_trait::async_trait]
impl CapacityRecoveryCommands for Commands {
    async fn status(
        &self,
        caller: &Caller,
        id: &AgentId,
        request: Option<&RecoveryRequest>,
    ) -> Result<RecoveryStatus, RecoveryError> {
        self.authorize(caller, id).await?;
        Ok(RecoveryStatus {
            agent_id: id.clone(),
            disposition: CapacityRecoveryDisposition::EvidenceRequired,
            phase: request.map(|_| RecoveryPhase::Completed),
            attempts: u32::from(request.is_some()),
            retry_at: None,
            operation_key: request.map(|r| r.key().unwrap()),
        })
    }
    async fn admit(
        &self,
        caller: &Caller,
        request: &RecoveryRequest,
    ) -> Result<CommitOutcome<RecoveryWork>, RecoveryError> {
        self.authorize(caller, &request.agent_id).await?;
        if self.mode == "disabled" {
            return Err(RecoveryError::Disabled);
        }
        self.outcome(request)
    }
    async fn resume(
        &self,
        caller: &Caller,
        request: &RecoveryRequest,
    ) -> Result<CommitOutcome<RecoveryWork>, RecoveryError> {
        self.authorize(caller, &request.agent_id).await?;
        self.outcome(request)
    }
}
fn completed(request: &RecoveryRequest) -> RecoveryWork {
    let now = &request.requested_at;
    RecoveryWork {
        format_version: 1,
        request: request.clone(),
        target: RecoveryTarget {
            agent_id: request.agent_id.clone(),
            creation_key: request.creation_key.clone(),
            creation_fingerprint: request.creation_fingerprint.clone(),
            conversation_id: "private-native".into(),
            conversation_generation: 1,
            runtime: RuntimeBinding {
                runtime_id: "private-runtime".into(),
                runtime_version: "fixture".into(),
                boot_id: Some("b".repeat(32)),
                provision_generation: Some(1),
                bound_at: now.clone(),
            },
            home_identity: "private-home".into(),
            original_open: AgentWorkReceipt {
                step: AgentWorkStep::OpenConversation,
                operation_id: "agent:agt_shared_read:open:v1".into(),
                request_sha256: "c".repeat(64),
                completed_at: now.clone(),
            },
            active_turn: None,
            input_sha256: "d".repeat(64),
        },
        phase: RecoveryPhase::Completed,
        revision: 3,
        attempts: 1,
        next_fence: 8,
        lease: None,
        ready_at: now.clone(),
        completed_at: Some(now.clone()),
    }
}
