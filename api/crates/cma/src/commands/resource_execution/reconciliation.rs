use super::*;
use futures_util::{stream, StreamExt};
use layer2_core::resource_access::execution::{
    EffectRecordV1, MemberPermitV1, MAX_EFFECT_OBSERVATIONS,
};
use std::time::Duration;

const RECONCILIATION_CONCURRENCY: usize = 4;
const LEDGER_READ_TIMEOUT: Duration = Duration::from_secs(5);
const STATUS_CONCURRENCY: usize = 16;
const STATUS_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone)]
pub(crate) struct ProviderEffectObservation {
    pub intent: wire::EffectIntentV1,
    pub outcome: wire::EffectOutcome,
    pub evidence_digest: wire::Digest,
}

/// Implementations authenticate status from the original provider for this exact
/// intent. Missing/unsupported lookup returns None or an error, never Rejected.
/// This port must not submit, retry, cancel or compensate a provider operation.
#[async_trait::async_trait]
pub(crate) trait EffectStatusProvider: Send + Sync {
    async fn status(
        &self,
        intent: &wire::EffectIntentV1,
    ) -> ExecutionResult<Option<ProviderEffectObservation>>;
}

pub(crate) struct EffectReconciler {
    store: Arc<dyn ResourceExecutionStore>,
    provider: Arc<dyn EffectStatusProvider>,
    status_slots: tokio::sync::Semaphore,
    reconciliation_slots: tokio::sync::Semaphore,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct EffectProgress {
    pub terminal: u32,
    pub unresolved: u32,
    pub failures: Vec<(wire::OpaqueId, ExecutionError)>,
}

impl EffectReconciler {
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "R3 awaits durable discovery and qualified status adapters"
        )
    )]
    pub(crate) fn new(
        store: Arc<dyn ResourceExecutionStore>,
        provider: Arc<dyn EffectStatusProvider>,
    ) -> Self {
        Self {
            store,
            provider,
            status_slots: tokio::sync::Semaphore::new(STATUS_CONCURRENCY),
            reconciliation_slots: tokio::sync::Semaphore::new(RECONCILIATION_CONCURRENCY),
        }
    }

    /// Reconcile the bounded ledger captured for a known durable registration.
    /// This does not discover registrations or establish revocation finality.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "R3 awaits durable discovery and qualified status adapters"
        )
    )]
    pub(crate) async fn reconcile(
        &self,
        registration: &wire::Digest,
    ) -> ExecutionResult<EffectProgress> {
        let _attempt = self
            .reconciliation_slots
            .try_acquire()
            .map_err(|_| ExecutionError::Capacity)?;
        let effects = tokio::time::timeout(LEDGER_READ_TIMEOUT, self.capture(registration))
            .await
            .unwrap_or(Err(ExecutionError::Unavailable))?;
        // Validate the full captured ledger before querying any external status.
        let mut results = stream::iter(effects.into_iter().map(|effect| async move {
            let id = effect.permission.intent.effect_id.clone();
            let result = self.observe(effect).await;
            (id, result)
        }))
        .buffer_unordered(STATUS_CONCURRENCY);
        let mut progress = EffectProgress {
            terminal: 0,
            unresolved: 0,
            failures: Vec::new(),
        };
        while let Some((id, result)) = results.next().await {
            match result {
                Ok(true) => progress.terminal += 1,
                Ok(false) => progress.unresolved += 1,
                Err(error) => {
                    progress.unresolved += 1;
                    progress.failures.push((id, error));
                }
            }
        }
        Ok(progress)
    }

    async fn capture(&self, registration: &wire::Digest) -> ExecutionResult<Vec<EffectRecordV1>> {
        let member = self
            .store
            .record(registration)
            .await?
            .ok_or(ExecutionError::Corrupt)?;
        member.validate()?;
        if &member.registration != registration {
            return Err(ExecutionError::EvidenceMismatch);
        }
        let Some(MemberPermitV1::Execution { ticket }) = &member.permit else {
            return Err(ExecutionError::EvidenceMismatch);
        };
        let mut effects = Vec::with_capacity(member.effect_count as usize);
        let mut identities = std::collections::HashSet::new();
        let mut after = 0;
        while after < member.effect_count {
            let limit = (member.effect_count - after).min(MAX_PAGE_MEMBERS);
            let page = self.store.effects_for(registration, after, limit).await?;
            if page.is_empty() || page.len() > limit as usize {
                return Err(ExecutionError::Corrupt);
            }
            for effect in page {
                effect.validate()?;
                effect.permission.intent.validate_for(ticket)?;
                if effect.permission.ledger_sequence.get() != u64::from(after + 1)
                    || !identities.insert(effect.permission.intent.effect_id.as_str().to_owned())
                {
                    return Err(ExecutionError::EvidenceMismatch);
                }
                after += 1;
                effects.push(effect);
            }
        }
        Ok(effects)
    }

    async fn observe(&self, effect: EffectRecordV1) -> ExecutionResult<bool> {
        if effect
            .observations
            .last()
            .is_some_and(|old| terminal(old.outcome))
        {
            return Ok(true);
        }
        let _slot = self
            .status_slots
            .acquire()
            .await
            .map_err(|_| ExecutionError::Unavailable)?;
        tokio::time::timeout(STATUS_TIMEOUT, self.refresh(effect))
            .await
            .unwrap_or(Err(ExecutionError::Unknown))
    }

    async fn refresh(&self, mut effect: EffectRecordV1) -> ExecutionResult<bool> {
        let previous = effect.observations.last();
        let status = self
            .provider
            .status(&effect.permission.intent)
            .await?
            .ok_or(ExecutionError::Unknown)?;
        if status.intent != effect.permission.intent
            || !matches!(
                status.outcome,
                wire::EffectOutcome::Submitted
                    | wire::EffectOutcome::CommitConfirmed
                    | wire::EffectOutcome::Rejected
                    | wire::EffectOutcome::ReconciliationRequired
            )
        {
            return Err(ExecutionError::EvidenceMismatch);
        }
        if previous.is_some_and(|old| {
            old.outcome == status.outcome && old.evidence_digest == status.evidence_digest
        }) {
            return Ok(false);
        }
        let resolved = terminal(status.outcome);
        if !resolved && effect.observations.len() >= MAX_EFFECT_OBSERVATIONS - 1 {
            return Err(ExecutionError::Capacity);
        }
        let observation = wire::EffectObservationV1 {
            protocol: wire::PROTOCOL_VERSION,
            permission_id: effect.permission.permission_id.clone(),
            execution: effect.permission.intent.execution.clone(),
            effect_id: effect.permission.intent.effect_id.clone(),
            sequence: wire::Generation::new(effect.observations.len() as u64 + 1)?,
            outcome: status.outcome,
            evidence_digest: status.evidence_digest,
        };
        effect.observe(&observation)?;
        // An uncertain/concurrent append is not success. The next pass reloads
        // the same ledger; it never recovers by reissuing an effect permission.
        self.store.observe_effect(&observation).await?;
        Ok(resolved)
    }
}

fn terminal(outcome: wire::EffectOutcome) -> bool {
    matches!(
        outcome,
        wire::EffectOutcome::CommitConfirmed | wire::EffectOutcome::Rejected
    )
}
