//! Bounded worker claim, ownership and continuation orchestration.

#[cfg(test)]
use super::super::lifecycle_failure::Phase;
use super::super::lifecycle_failure::{LifecycleAttempt, LifecycleTaskFailure, Origin};
use super::startup_holds::{Admission, ProbePermit};
use super::*;

impl AgentWorker {
    /// Keep the startup/cleanup execution state machine behind a boxed boundary
    /// so callers do not inherit its large stack frame during fixture startup.
    #[cfg(test)]
    pub fn run_one(
        &self,
        id: &AgentId,
        now: SystemTime,
    ) -> Pin<Box<dyn Future<Output = Result<bool, SandboxStoreError>> + Send + '_>> {
        let id = id.clone();
        Box::pin(async move {
            let attempt = LifecycleAttempt::new(Phase::Execution, Some(&id));
            self.run_one_observed(&id, now, &attempt)
                .await
                .map_err(LifecycleTaskFailure::into_compatibility)
        })
    }

    #[cfg(test)]
    pub(in crate::sandbox) fn run_one_diagnostic(
        &self,
        id: &AgentId,
        now: SystemTime,
    ) -> Pin<Box<dyn Future<Output = Result<bool, LifecycleTaskFailure>> + Send + '_>> {
        let id = id.clone();
        Box::pin(async move {
            let attempt = LifecycleAttempt::new(Phase::Execution, Some(&id));
            self.run_one_observed(&id, now, &attempt).await
        })
    }

    async fn run_one_observed(
        &self,
        id: &AgentId,
        now: SystemTime,
        attempt: &LifecycleAttempt,
    ) -> Result<bool, LifecycleTaskFailure> {
        let startup_probe = match self.startup_admission(id, None, true).await {
            Admission::Proceed => None,
            Admission::Probe(permit) => Some(permit),
            Admission::Hold => return Ok(false),
        };
        if let Some(coordinator) = &self.config.ownership {
            use super::super::ownership_retry_hint::{AdvisoryDecision, CapturedOwnershipProbe};
            if let Some(hints) = &self.config.ownership_retry_hints {
                if hints.preflight(id).await == AdvisoryDecision::Hold {
                    return Ok(false);
                }
            }
            if self
                .dependencies
                .store
                .agent_inspection_parked(id)
                .await
                .map_err(|error| attempt.store(Origin::Claim, error))?
            {
                return Ok(false);
            }
            let Some(authority) = self
                .acquire_pending_work(coordinator, id)
                .await
                .map_err(|error| attempt.ownership(Origin::OwnershipAcquire, error))?
            else {
                return Ok(false);
            };
            let captured: OnceLock<Arc<dyn CapturedOwnershipProbe>> = OnceLock::new();
            let evidence = self
                .config
                .ownership_retry_hints
                .as_ref()
                .and_then(|_| authority.observe_retry_evidence());
            let inspection = self
                .dependencies
                .store
                .agent_inspection_parking_enabled()
                .then(|| Arc::new(OnceLock::new()));
            let outcome = authority
                .run(inspection_park::ATTEMPT.scope(
                    inspection.clone(),
                    Box::pin(self.run_claimed(
                        id,
                        now,
                        attempt,
                        Some((&authority.token, &captured)),
                        startup_probe.as_ref(),
                    )),
                ))
                .await;
            if let Some(inspection) =
                inspection.filter(|_| outcome.is_err() && authority.receiver_cancelled())
            {
                if self
                    .park_failed_inspection(attempt, &authority.token, &inspection)
                    .await?
                {
                    return Ok(false);
                }
            }
            if let (Some(observed), Some(failure)) = (
                captured.get(),
                evidence.and_then(|evidence| evidence.first_failure()),
            ) {
                let _ = observed.record(&failure).await;
            }
            let result =
                outcome.map_err(|error| attempt.ownership(Origin::AuthorityScope, error))?;
            if result.is_ok() {
                self.finish_ownership_attempt(id, coordinator, &authority, attempt)
                    .await?;
            }
            return result;
        }
        Box::pin(self.run_claimed(id, now, attempt, None, startup_probe.as_ref())).await
    }

    async fn run_claimed(
        &self,
        id: &AgentId,
        now: SystemTime,
        attempt: &LifecycleAttempt,
        observation: Option<(
            &layer2_core::agent_ownership::OwnershipToken,
            &OnceLock<Arc<dyn super::super::ownership_retry_hint::CapturedOwnershipProbe>>,
        )>,
        startup_probe: Option<&ProbePermit<'_>>,
    ) -> Result<bool, LifecycleTaskFailure> {
        if observation.is_some()
            && matches!(
                self.startup_admission(id, startup_probe, false).await,
                Admission::Hold
            )
        {
            return Ok(false);
        }
        let started = tokio::time::Instant::now();
        let Some(agent) = self
            .dependencies
            .store
            .claim_agent_work_observed(
                id,
                &self.config.holder,
                rfc3339_at(now),
                rfc3339_at(now + self.config.lease_duration),
            )
            .await
            .map_err(|error| attempt.claim(error))?
        else {
            return Ok(false);
        };
        let claim_ms = started.elapsed().as_millis() as u64;
        if let (Some(hints), Some((owner, captured))) =
            (&self.config.ownership_retry_hints, observation)
        {
            if let Some(observed) = hints.capture(&agent, owner).await {
                let _ = captured.set(observed);
            }
        }
        let step = agent.work.step;
        let executing = tokio::time::Instant::now();
        let result = Box::pin(self.execute(agent, now + started.elapsed())).await;
        tracing::info!(
            agent_id = %id,
            ?step,
            claim_ms,
            execution_ms = executing.elapsed().as_millis() as u64,
            published = result.is_ok(),
            "Agent work attempt completed"
        );
        result.map_err(|error| attempt.store(Origin::Execute, error))?;
        Ok(true)
    }

    /// Follow bounded, committed progress immediately, keeping each durable claim.
    #[cfg(test)]
    pub(crate) async fn run_scheduled(
        &self,
        id: &AgentId,
        now: SystemTime,
    ) -> Result<bool, SandboxStoreError> {
        let attempt = LifecycleAttempt::new(Phase::Execution, Some(id));
        self.run_scheduled_observed(id, now, &attempt)
            .await
            .map_err(LifecycleTaskFailure::into_compatibility)
    }

    pub(in crate::sandbox) async fn run_scheduled_observed(
        &self,
        id: &AgentId,
        now: SystemTime,
        attempt: &LifecycleAttempt,
    ) -> Result<bool, LifecycleTaskFailure> {
        // A queue may predate the latest receiver failure. Deferral confers no
        // claim and leaves the durable intent for the next periodic discovery.
        if self.defer_ownership_retry(id).await {
            return Ok(false);
        }
        let started = tokio::time::Instant::now();
        let progressed = Box::pin(self.run_one_observed(id, now, attempt)).await?;
        if !progressed {
            return Ok(false);
        }
        for index in 1..=6 {
            attempt.continuation(index);
            let Some(agent) = self
                .dependencies
                .store
                .get_agent(id)
                .await
                .map_err(|error| attempt.store(Origin::ContinuationRead, error))?
            else {
                break;
            };
            let startup = matches!(
                agent.work.step,
                AgentWorkStep::AddWorktree
                    | AgentWorkStep::MaterializeHome
                    | AgentWorkStep::OpenConversation
                    | AgentWorkStep::PublishReady
                    | AgentWorkStep::PublishConnection
                    | AgentWorkStep::DispatchFirstMessage
            );
            let cleanup = self.config.cleanup_continuation_enabled
                && agent.work.attempts == 0
                && matches!(
                    agent.work.step,
                    AgentWorkStep::FenceInteraction
                        | AgentWorkStep::CloseProcess
                        | AgentWorkStep::PublishStopped
                        | AgentWorkStep::EnsureProcessClosed
                        | AgentWorkStep::RemoveHome
                        | AgentWorkStep::RemoveWorktree
                        | AgentWorkStep::PublishDeleted
                );
            // A published retry is not progress, even when execution exceeded its
            // deadline. Reclaim only committed successful cleanup transitions.
            if !(startup || cleanup) || !agent.work.is_due(&rfc3339_at(now + started.elapsed())) {
                break;
            }
            if !Box::pin(self.run_one_observed(id, now + started.elapsed(), attempt)).await? {
                break;
            }
        }
        Ok(true)
    }
}
