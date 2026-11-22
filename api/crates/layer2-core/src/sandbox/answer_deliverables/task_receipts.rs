use crate::{ids::AgentCommandId, now_rfc3339, AgentId};

use super::super::{store::SandboxStoreError, task_acceptance::TaskAcceptanceSummary};
use super::{AnswerDeliverableService, DeliverableAccess, DeliverableError};

impl AnswerDeliverableService {
    pub async fn confirm_task_delivery(
        &self,
        access: &DeliverableAccess<'_>,
        agent: &AgentId,
        task_id: &AgentCommandId,
        answer: u64,
        delivery: &str,
    ) -> Result<TaskAcceptanceSummary, DeliverableError> {
        let store = self.task_store.as_ref().ok_or(DeliverableError::NotFound)?;
        self.reads
            .require_owned_final_answer(access.caller, agent, answer)
            .await?;
        let reading = self
            .read_item(access, agent, answer, delivery, true)
            .await?;
        let verified_at = now_rfc3339();
        for _ in 0..3 {
            let mut next = self.reads.read(access.caller, agent).await?;
            let task = next
                .conversation
                .observations
                .task_acceptance
                .as_mut()
                .filter(|task| {
                    &task.task_id == task_id && task.turn_sequence == reading.turn_sequence
                })
                .ok_or(DeliverableError::NotFound)?;
            if task.confirmed(&reading) {
                return Ok(task.summary());
            }
            if !task.accept_delivery(&reading, &verified_at) {
                return Err(DeliverableError::NotFound);
            }
            let outcome = task.summary();
            let revision = next.revision;
            next.revision = revision
                .checked_add(1)
                .ok_or(DeliverableError::Unavailable)?;
            match store.save_agent(revision, next).await {
                Ok(()) => {
                    self.reads.read(access.caller, agent).await?;
                    return Ok(outcome);
                }
                Err(SandboxStoreError::Conflict) => continue,
                Err(SandboxStoreError::NotFound) => return Err(DeliverableError::NotFound),
                Err(_) => {
                    tracing::warn!("Persisting verified task delivery failed");
                    return Err(DeliverableError::Unavailable);
                }
            }
        }
        Err(DeliverableError::Unavailable)
    }
}
