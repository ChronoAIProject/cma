use super::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectRecordV1 {
    pub protocol: u32,
    pub permission: wire::EffectPermissionV1,
    pub observation: Option<wire::EffectObservationV1>,
}

impl EffectRecordV1 {
    pub fn validate(&self) -> ExecutionResult<()> {
        wire::version(self.protocol)?;
        wire::version(self.permission.protocol)?;
        self.permission.intent.grant.validate()?;
        let member = wire::MemberKey::Execution {
            execution: self.permission.intent.execution.clone(),
        };
        if registration_id(&member)? != self.permission.intent.registration {
            return Err(ExecutionError::Corrupt);
        }
        if self.permission.ledger_sequence.get() > u64::from(MAX_MEMBER_EFFECTS)
            || self.permission.permission_id
                != digest(&("resource-effect-permission-v1", &self.permission.intent))?
        {
            return Err(ExecutionError::Corrupt);
        }
        if let Some(observation) = &self.observation {
            observation.validate_for(&self.permission)?;
        }
        Ok(())
    }

    pub fn replay_prepared(
        &self,
        intent: &wire::EffectIntentV1,
    ) -> ExecutionResult<wire::EffectPermissionV1> {
        self.validate()?;
        if &self.permission.intent != intent {
            return Err(ExecutionError::IdempotencyConflict);
        }
        // A retained result is evidence only. Only the original pending
        // permission can be recovered, and its receiver still consumes it once.
        if self.observation.is_some() {
            return Err(ExecutionError::Unknown);
        }
        Ok(self.permission.clone())
    }

    pub fn observe(&mut self, observation: &wire::EffectObservationV1) -> ExecutionResult<()> {
        use wire::EffectOutcome as O;
        self.validate()?;
        observation.validate_for(&self.permission)?;
        if let Some(previous) = &self.observation {
            if previous == observation {
                return Ok(());
            }
            if observation.sequence != previous.sequence.successor()? {
                return Err(ExecutionError::EvidenceMismatch);
            }
            let allowed = match previous.outcome {
                O::SubmissionUnknown | O::ReconciliationRequired => matches!(
                    observation.outcome,
                    O::Submitted | O::CommitConfirmed | O::Rejected | O::ReconciliationRequired
                ),
                O::Submitted => matches!(
                    observation.outcome,
                    O::CommitConfirmed | O::Rejected | O::ReconciliationRequired
                ),
                O::Prepared | O::CommitConfirmed | O::Rejected => false,
            };
            if !allowed {
                return Err(ExecutionError::EvidenceMismatch);
            }
        } else if observation.sequence.get() != 1 {
            return Err(ExecutionError::EvidenceMismatch);
        }
        self.observation = Some(observation.clone());
        Ok(())
    }
}

impl ResourceExecutionCoordinator {
    pub async fn prepare_effect(
        &self,
        authenticated: &wire::ReceiverKey,
        intent: &wire::EffectIntentV1,
    ) -> ExecutionResult<wire::EffectPermissionV1> {
        if authenticated != &intent.execution.receiver {
            return Err(ExecutionError::EvidenceMismatch);
        }
        if !self.policy.admitting && intent.grant.family().is_some() {
            return Err(ExecutionError::Disabled);
        }
        self.policy.store.prepare_effect(intent).await
    }

    pub async fn observe_effect(
        &self,
        authenticated: &wire::ReceiverKey,
        observation: &wire::EffectObservationV1,
    ) -> ExecutionResult<()> {
        if authenticated != &observation.execution.receiver {
            return Err(ExecutionError::EvidenceMismatch);
        }
        self.policy.store.observe_effect(observation).await
    }
}
