use std::time::SystemTime;

use layer3_adapter::{AdapterError, ConversationUpdate, RuntimeOutputSource};

use crate::sandbox::ingress_ownership::{
    AdmissionRefusal, AdmittedError, AdmittedOperation, IngressOwnershipPolicy, IngressWorkScope,
    OwnedWork, RecordingSession,
};
use crate::sandbox::store::AgentReopenGuard;
use crate::sandbox::{
    AgentConversationError, AgentOutputCommit, AgentUpdateOutcome, AgentUpdateRecorder,
};
use crate::{rfc3339_at, AgentId};

// Concrete futures keep branch selection outside async-trait's heap allocation.
impl dyn AgentUpdateRecorder + '_ {
    pub async fn restore_recording(
        &self,
        id: &AgentId,
        generation: u64,
        session: &mut RecordingSession,
    ) -> Result<OwnedWork<()>, OwnedWork<AgentConversationError>> {
        let Some(policy) = session.policy().cloned() else {
            return self.restore_session(id, generation, session).await;
        };
        resolve(
            self.restore_session_operation(id, generation, session),
            &policy,
        )
        .await
    }

    pub async fn record_update_in_session(
        &self,
        id: &AgentId,
        generation: u64,
        session: &mut RecordingSession,
        update: &ConversationUpdate,
        appended_at: SystemTime,
    ) -> Result<OwnedWork<AgentUpdateOutcome>, OwnedWork<AgentConversationError>> {
        let Some(policy) = session.policy().cloned() else {
            return self
                .record_session_update(id, generation, session, update, rfc3339_at(appended_at))
                .await;
        };
        resolve(
            self.record_session_update_operation(id, generation, session, update, appended_at),
            &policy,
        )
        .await
    }

    pub async fn record_output_in_session(
        &self,
        id: &AgentId,
        generation: u64,
        session: &mut RecordingSession,
        update: &ConversationUpdate,
        appended_at: SystemTime,
    ) -> Result<OwnedWork<AgentOutputCommit>, OwnedWork<AgentConversationError>> {
        let Some(policy) = session.policy().cloned() else {
            return self
                .record_session_output(id, generation, session, update, rfc3339_at(appended_at))
                .await;
        };
        resolve(
            self.record_session_output_operation(id, generation, session, update, appended_at),
            &policy,
        )
        .await
    }

    pub async fn record_reopen_in_session(
        &self,
        id: &AgentId,
        guard: &AgentReopenGuard,
        session: &mut RecordingSession,
        update: &ConversationUpdate,
        appended_at: SystemTime,
    ) -> Result<OwnedWork<AgentUpdateOutcome>, OwnedWork<AgentConversationError>> {
        let Some(policy) = session.policy().cloned() else {
            return self
                .record_session_reopen(id, guard, session, update, rfc3339_at(appended_at))
                .await;
        };
        resolve(
            self.record_session_reopen_operation(id, guard, session, update, appended_at),
            &policy,
        )
        .await
    }

    pub async fn flush_recording(
        &self,
        id: &AgentId,
        generation: u64,
        session: &mut RecordingSession,
        appended_at: SystemTime,
    ) -> Result<OwnedWork<Option<RuntimeOutputSource>>, OwnedWork<AgentConversationError>> {
        let Some(policy) = session.policy().cloned() else {
            return self
                .flush_session_output(id, generation, session, rfc3339_at(appended_at))
                .await;
        };
        resolve(
            self.flush_session_output_operation(id, generation, session, appended_at),
            &policy,
        )
        .await
    }
}

async fn resolve<T>(
    operation: Result<AdmittedOperation<'_, Result<T, AgentConversationError>>, AdmittedError>,
    policy: &IngressOwnershipPolicy,
) -> Result<OwnedWork<T>, OwnedWork<AgentConversationError>> {
    let result = match operation {
        Ok(operation) if operation.belongs_to(policy) => operation.await,
        Ok(_) => Err(AdmissionRefusal::InvalidPlan {
            phase: "recording operation ownership",
        }
        .into()),
        Err(error) => Err(error),
    };
    match result {
        Ok(result) => result.transpose(),
        Err(error) => Err(IngressWorkScope::inherit_owned(async move {
            // Classification borrows the source through its final inspection.
            // No diagnostic String escapes its owner on this metadata-only path.
            match &error {
                AdmittedError::Refused(AdmissionRefusal::AtCapacity { .. }) => {
                    AgentConversationError::CapacityExceeded
                }
                AdmittedError::Refused(AdmissionRefusal::InvalidPlan { .. }) => {
                    AgentConversationError::InvalidState("invalid recording allocation plan")
                }
                AdmittedError::Unsupported(detail) => AgentConversationError::InvalidState(detail),
                AdmittedError::Adapter(error)
                    if matches!(&**error, AdapterError::AtCapacity(_)) =>
                {
                    AgentConversationError::CapacityExceeded
                }
                AdmittedError::Adapter(_) | AdmittedError::Unavailable(_) => {
                    AgentConversationError::Unavailable(String::new())
                }
            }
        })
        .await),
    }
}

#[cfg(test)]
#[path = "ingress_tests.rs"]
mod tests;
