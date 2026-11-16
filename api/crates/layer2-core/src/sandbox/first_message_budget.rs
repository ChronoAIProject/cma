use super::*;

impl AgentOperationService {
    pub(super) async fn refuse_first_message_admission(
        &self,
        current: &Agent,
        authority: &AgentWorkAuthority,
        reason: crate::admission::AdmissionRefusal,
    ) -> Result<FirstMessageDispatchOutcome, AgentOperationError> {
        require_work_authority(current, authority)?;
        let at = now_rfc3339();
        let mut next = current.clone();
        let record = next
            .conversation
            .first_message
            .as_mut()
            .ok_or_else(|| AgentOperationError::Corrupt("first message disappeared".into()))?;
        match reason {
            crate::admission::AdmissionRefusal::Budget(reason) => {
                record.refuse_budget(reason, at.clone())
            }
            crate::admission::AdmissionRefusal::Capacity(
                crate::admission::CapacityKind::ActiveSessions,
            ) => record.refuse_session_capacity(at.clone()),
            _ => {
                return Err(AgentOperationError::Corrupt(
                    "invalid first message refusal".into(),
                ))
            }
        }
        .map_err(|error| AgentOperationError::Corrupt(error.to_string()))?;
        let refused = record.clone();
        self.commit_change(current, next, vec![first_message_log(&refused)], at)
            .await?;
        Ok(FirstMessageDispatchOutcome::Accepted(refused))
    }
}
