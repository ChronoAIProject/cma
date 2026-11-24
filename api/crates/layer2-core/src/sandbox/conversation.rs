use layer3_adapter::ConversationUpdate;

use crate::ids::AgentId;

use super::ingress_ownership::{
    AdmittedError, AdmittedOperation, IngressWorkScope, OwnedWork, RecordingSession,
};
use super::AgentLogEntry;

pub const MAX_DURABLE_AGENT_MESSAGE_BYTES: usize = 1024 * 1024;

/// Live message fragments for one runtime generation.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentMessageAssembly {
    pub(crate) message_id: Option<String>,
    pub(crate) content: String,
    pub(crate) prompt_evidence: bool,
    pub(crate) live_message: Option<super::conversation_recorder::live::AgentLiveMessageIdentity>,
    pub(crate) live_thought: Option<super::conversation_recorder::live::AgentLiveMessageIdentity>,
    #[serde(skip)]
    pub(crate) pending_update: Option<Box<PendingAgentUpdate>>,
    #[serde(skip)]
    pub(crate) output_state: Option<Box<super::conversation_recorder::coalesce::OutputState>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PendingAgentUpdate {
    pub id: String,
    pub fingerprint: String,
    pub native_conversation_id: String,
    pub runtime_generation: u64,
    pub active_turn: Option<u64>,
    pub after_seq: u64,
}

impl AgentMessageAssembly {
    pub fn is_empty(&self) -> bool {
        self.content.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AgentUpdateOutcome {
    pub appended: Vec<AgentLogEntry>,
    pub turn_closed: bool,
}

impl AgentUpdateOutcome {
    pub(crate) fn quiet() -> Self {
        Self {
            appended: Vec::new(),
            turn_closed: false,
        }
    }
}

/// Runtime-only commit receipt: only this source is eligible for cumulative ACK.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentOutputCommit {
    pub outcome: AgentUpdateOutcome,
    pub committed: Option<layer3_adapter::RuntimeOutputSource>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AgentConversationError {
    #[error("agent not found")]
    NotFound,
    #[error("agent runtime generation is stale")]
    StaleGeneration,
    #[error("agent is not in a valid conversation state: {0}")]
    InvalidState(&'static str),
    #[error("agent conversation data is not usable: {0}")]
    Corrupt(String),
    #[error("agent conversation data is not usable: runtime output receipt is not contiguous with its conversation")]
    ReopenOutputDiscontinuity,
    #[error("agent conversation log capacity exceeded")]
    CapacityExceeded,
    #[error("agent conversation store is unavailable: {0}")]
    Unavailable(String),
}

#[async_trait::async_trait]
pub trait AgentUpdateRecorder: Send + Sync {
    fn recording_session(&self) -> RecordingSession {
        RecordingSession::new(None)
    }

    /// Admit storage before constructing an erased future, copying inputs or
    /// formatting timestamps. Implementations retain each result and error.
    fn restore_session_operation<'a>(
        &'a self,
        _id: &'a AgentId,
        _runtime_generation: u64,
        _session: &'a mut RecordingSession,
    ) -> Result<AdmittedOperation<'a, Result<(), AgentConversationError>>, AdmittedError> {
        Err(AdmittedError::Unsupported(
            "admitted assembly restoration is unsupported",
        ))
    }

    fn record_session_update_operation<'a>(
        &'a self,
        _id: &'a AgentId,
        _runtime_generation: u64,
        _session: &'a mut RecordingSession,
        _update: &'a ConversationUpdate,
        _appended_at: std::time::SystemTime,
    ) -> Result<
        AdmittedOperation<'a, Result<AgentUpdateOutcome, AgentConversationError>>,
        AdmittedError,
    > {
        Err(AdmittedError::Unsupported(
            "admitted update recording is unsupported",
        ))
    }

    fn record_session_output_operation<'a>(
        &'a self,
        _id: &'a AgentId,
        _runtime_generation: u64,
        _session: &'a mut RecordingSession,
        _update: &'a ConversationUpdate,
        _appended_at: std::time::SystemTime,
    ) -> Result<
        AdmittedOperation<'a, Result<AgentOutputCommit, AgentConversationError>>,
        AdmittedError,
    > {
        Err(AdmittedError::Unsupported(
            "admitted output recording is unsupported",
        ))
    }

    fn record_session_reopen_operation<'a>(
        &'a self,
        _id: &'a AgentId,
        _guard: &'a super::store::AgentReopenGuard,
        _session: &'a mut RecordingSession,
        _update: &'a ConversationUpdate,
        _appended_at: std::time::SystemTime,
    ) -> Result<
        AdmittedOperation<'a, Result<AgentUpdateOutcome, AgentConversationError>>,
        AdmittedError,
    > {
        Err(AdmittedError::Unsupported(
            "admitted reopen recording is unsupported",
        ))
    }

    fn flush_session_output_operation<'a>(
        &'a self,
        _id: &'a AgentId,
        _runtime_generation: u64,
        _session: &'a mut RecordingSession,
        _appended_at: std::time::SystemTime,
    ) -> Result<
        AdmittedOperation<
            'a,
            Result<Option<layer3_adapter::RuntimeOutputSource>, AgentConversationError>,
        >,
        AdmittedError,
    > {
        Err(AdmittedError::Unsupported(
            "admitted output flush is unsupported",
        ))
    }

    /// Legacy implementations preserve their algorithm only for an unowned
    /// session. Implementations of the admitted path must own both outcomes and
    /// errors through their caller's final use, including receipt acknowledgement.
    async fn restore_session(
        &self,
        id: &AgentId,
        runtime_generation: u64,
        session: &mut RecordingSession,
    ) -> Result<OwnedWork<()>, OwnedWork<AgentConversationError>> {
        IngressWorkScope::inherit_owned(async {
            let assembly =
                session
                    .legacy_assembly_mut()
                    .ok_or(AgentConversationError::InvalidState(
                        "admitted assembly restoration is unsupported",
                    ))?;
            self.restore_assembly(id, runtime_generation, assembly)
                .await
        })
        .await
        .transpose()
    }

    async fn record_session_update(
        &self,
        id: &AgentId,
        runtime_generation: u64,
        session: &mut RecordingSession,
        update: &ConversationUpdate,
        appended_at: String,
    ) -> Result<OwnedWork<AgentUpdateOutcome>, OwnedWork<AgentConversationError>> {
        IngressWorkScope::inherit_owned(async {
            let assembly =
                session
                    .legacy_assembly_mut()
                    .ok_or(AgentConversationError::InvalidState(
                        "admitted update recording is unsupported",
                    ))?;
            self.record_update(
                id,
                runtime_generation,
                assembly,
                update.clone(),
                appended_at,
            )
            .await
        })
        .await
        .transpose()
    }

    async fn record_session_output(
        &self,
        id: &AgentId,
        runtime_generation: u64,
        session: &mut RecordingSession,
        update: &ConversationUpdate,
        appended_at: String,
    ) -> Result<OwnedWork<AgentOutputCommit>, OwnedWork<AgentConversationError>> {
        IngressWorkScope::inherit_owned(async {
            let assembly =
                session
                    .legacy_assembly_mut()
                    .ok_or(AgentConversationError::InvalidState(
                        "admitted output recording is unsupported",
                    ))?;
            self.record_output(
                id,
                runtime_generation,
                assembly,
                update.clone(),
                appended_at,
            )
            .await
        })
        .await
        .transpose()
    }

    async fn record_session_reopen(
        &self,
        id: &AgentId,
        guard: &super::store::AgentReopenGuard,
        session: &mut RecordingSession,
        update: &ConversationUpdate,
        appended_at: String,
    ) -> Result<OwnedWork<AgentUpdateOutcome>, OwnedWork<AgentConversationError>> {
        IngressWorkScope::inherit_owned(async {
            let assembly =
                session
                    .legacy_assembly_mut()
                    .ok_or(AgentConversationError::InvalidState(
                        "admitted reopen recording is unsupported",
                    ))?;
            self.record_reopen_update(id, guard, assembly, update.clone(), appended_at)
                .await
        })
        .await
        .transpose()
    }

    async fn flush_session_output(
        &self,
        id: &AgentId,
        runtime_generation: u64,
        session: &mut RecordingSession,
        appended_at: String,
    ) -> Result<
        OwnedWork<Option<layer3_adapter::RuntimeOutputSource>>,
        OwnedWork<AgentConversationError>,
    > {
        IngressWorkScope::inherit_owned(async {
            let assembly =
                session
                    .legacy_assembly_mut()
                    .ok_or(AgentConversationError::InvalidState(
                        "admitted output flush is unsupported",
                    ))?;
            self.flush_output(id, runtime_generation, assembly, appended_at)
                .await
        })
        .await
        .transpose()
    }

    async fn budget_stop(
        &self,
        _id: &AgentId,
        _runtime_generation: u64,
    ) -> Result<Option<crate::budget::BudgetStop>, AgentConversationError> {
        Ok(None)
    }
    async fn record_reopen_update(
        &self,
        _id: &AgentId,
        _guard: &super::store::AgentReopenGuard,
        _assembly: &mut AgentMessageAssembly,
        _update: ConversationUpdate,
        _appended_at: String,
    ) -> Result<AgentUpdateOutcome, AgentConversationError> {
        Err(AgentConversationError::InvalidState(
            "durable reopen recording is unsupported",
        ))
    }

    fn subscribe_live(&self, _id: &AgentId) -> Option<super::AgentLiveSubscription> {
        None
    }

    async fn admit_live(
        &self,
        id: &AgentId,
    ) -> Result<Option<super::AgentLiveSubscription>, super::AgentLiveError> {
        let mut subscription = self.subscribe_live(id);
        if let Some(subscription) = subscription.as_mut() {
            subscription.admit().await?;
        }
        Ok(subscription)
    }

    /// A local hint only. Subscribers must read and authorize durable state.
    fn subscribe_head(&self, _id: &AgentId) -> Option<super::AgentHeadSubscription> {
        None
    }

    async fn restore_assembly(
        &self,
        _id: &AgentId,
        _runtime_generation: u64,
        _assembly: &mut AgentMessageAssembly,
    ) -> Result<(), AgentConversationError> {
        Ok(())
    }

    /// Ordered live consumer entry point; implementations may defer only quiet receipts.
    async fn record_output(
        &self,
        id: &AgentId,
        runtime_generation: u64,
        assembly: &mut AgentMessageAssembly,
        update: ConversationUpdate,
        appended_at: String,
    ) -> Result<AgentOutputCommit, AgentConversationError> {
        let source = update
            .delivery()
            .receipt
            .map(|receipt| receipt.source.clone());
        let outcome = self
            .record_update(id, runtime_generation, assembly, update, appended_at)
            .await?;
        Ok(AgentOutputCommit {
            outcome,
            committed: source,
        })
    }

    async fn flush_output(
        &self,
        _id: &AgentId,
        _runtime_generation: u64,
        _assembly: &mut AgentMessageAssembly,
        _appended_at: String,
    ) -> Result<Option<layer3_adapter::RuntimeOutputSource>, AgentConversationError> {
        Ok(None)
    }

    async fn record_update(
        &self,
        id: &AgentId,
        runtime_generation: u64,
        assembly: &mut AgentMessageAssembly,
        update: ConversationUpdate,
        appended_at: String,
    ) -> Result<AgentUpdateOutcome, AgentConversationError>;
}
