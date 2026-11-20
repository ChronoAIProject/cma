use super::*;

pub const MAX_EFFECT_OBSERVATIONS: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectRecordV1 {
    pub protocol: u32,
    pub permission: wire::EffectPermissionV1,
    pub observations: Vec<wire::EffectObservationV1>,
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
        if self.observations.len() > MAX_EFFECT_OBSERVATIONS {
            return Err(ExecutionError::Corrupt);
        }
        for (index, observation) in self.observations.iter().enumerate() {
            observation.validate_for(&self.permission)?;
            if observation.sequence.get() != index as u64 + 1
                || (index == MAX_EFFECT_OBSERVATIONS - 1 && !terminal(observation.outcome))
                || (index > 0
                    && !follows(self.observations[index - 1].outcome, observation.outcome))
            {
                return Err(ExecutionError::Corrupt);
            }
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
        if !self.observations.is_empty() {
            return Err(ExecutionError::Unknown);
        }
        Ok(self.permission.clone())
    }

    pub fn observe(&mut self, observation: &wire::EffectObservationV1) -> ExecutionResult<()> {
        self.validate()?;
        observation.validate_for(&self.permission)?;
        if let Some(previous) = self
            .observations
            .iter()
            .find(|old| old.sequence == observation.sequence)
        {
            if previous == observation {
                return Ok(());
            }
            return Err(ExecutionError::EvidenceMismatch);
        }
        if observation.sequence.get() != self.observations.len() as u64 + 1
            || self
                .observations
                .last()
                .is_some_and(|old| !follows(old.outcome, observation.outcome))
        {
            return Err(ExecutionError::EvidenceMismatch);
        }
        // Nonterminal observations cannot consume the reserved final-result
        // slot. Full evidence history never participates in revoke admission.
        if self.observations.len() >= MAX_EFFECT_OBSERVATIONS
            || (self.observations.len() == MAX_EFFECT_OBSERVATIONS - 1
                && !terminal(observation.outcome))
        {
            return Err(ExecutionError::Capacity);
        }
        self.observations.push(observation.clone());
        Ok(())
    }
}

fn terminal(outcome: wire::EffectOutcome) -> bool {
    matches!(
        outcome,
        wire::EffectOutcome::CommitConfirmed | wire::EffectOutcome::Rejected
    )
}

fn follows(previous: wire::EffectOutcome, next: wire::EffectOutcome) -> bool {
    use wire::EffectOutcome as O;
    match previous {
        O::SubmissionUnknown | O::ReconciliationRequired => matches!(
            next,
            O::Submitted | O::CommitConfirmed | O::Rejected | O::ReconciliationRequired
        ),
        O::Submitted => matches!(
            next,
            O::CommitConfirmed | O::Rejected | O::ReconciliationRequired
        ),
        O::Prepared | O::CommitConfirmed | O::Rejected => false,
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
