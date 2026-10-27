//! Periodic bounded discovery and cancellable, renewable worker claims.
use super::*;
use crate::store::HEARTBEAT;

// Warm creation should observe committed readiness promptly. Bound the faster
// polling window; sequential reads and the worker-slot limit cap its load.
pub(super) fn readiness_delay(started: tokio::time::Instant) -> Duration {
    if started.elapsed() < Duration::from_secs(10) {
        Duration::from_millis(100)
    } else {
        Duration::from_millis(500)
    }
}

impl InlineAgentLifecycle {
    pub(crate) fn resume_pending(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(10));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tick.tick().await;
                let Some(this) = weak.upgrade() else {
                    return;
                };
                match this.operations.pending().await {
                    Ok(ids) => {
                        for id in ids {
                            this.spawn_worker(id);
                        }
                    }
                    Err(error) => {
                        tracing::error!(error = %error, "cannot discover Agent lifecycle operations")
                    }
                }
            }
        });
    }

    pub(super) async fn wait_for_agent_ready(
        &self,
        caller: &Caller,
        id: &AgentId,
    ) -> Result<(), AgentLifecycleProvisioningError> {
        use layer2_core::sandbox::AgentStatus;
        tokio::time::timeout(Duration::from_secs(180), async {
            let started = tokio::time::Instant::now();
            loop {
                let agent = self.agents.read(caller, id).await.map_err(agent_error)?;
                match agent.status {
                    AgentStatus::Starting => tokio::time::sleep(readiness_delay(started)).await,
                    AgentStatus::Ready
                    | AgentStatus::Working
                    | AgentStatus::WaitingApproval
                    | AgentStatus::Paused => return Ok(()),
                    _ => {
                        return Err(AgentLifecycleProvisioningError::Conflict(
                            "Agent runtime preparation failed".into(),
                        ))
                    }
                }
            }
        })
        .await
        .map_err(|_| AgentLifecycleProvisioningError::Unavailable)?
    }

    pub(super) fn spawn_worker(&self, operation_id: String) {
        let Some(this) = self.self_ref.get().and_then(Weak::upgrade) else {
            return;
        };
        let Ok(slot) = this.worker_slots.clone().try_acquire_owned() else {
            return;
        };
        tokio::spawn(async move {
            let _slot = slot;
            if let Err(error) = this.run_worker(&operation_id).await {
                tracing::error!(operation = %operation_id, error = %error, "Agent lifecycle worker lost storage or claim; discovery will retry");
            }
        });
    }

    async fn run_worker(&self, operation_id: &str) -> Result<(), layer2_core::StoreError> {
        let Some(record) = self.operations.claim(operation_id).await? else {
            return Ok(());
        };
        let lease = record.lease()?;
        let attempts = record.attempts;
        let capacity = record.input.captured_capacity(self.bounds);
        let limits = record.input.captured_limits();
        let first_message = validation::admitted_first_message(&record.input);
        let request = guidance::provisioning_request(
            &record.input.request,
            record.input.managed_agent_instructions.as_deref(),
        );
        let caller = Caller::new(
            SubjectId::new(record.owner),
            Authorization::new(record.input.capabilities),
        );
        let work = self.provision(
            &caller,
            ProvisionInput {
                behavior: record.input.behavior,
                first_message,
                request,
                runtime_image: record.input.resolved_image,
                capacity,
                limits,
                sandbox_availability: record.input.sandbox_availability,
                key: record.key,
                lease: lease.clone(),
                existing_sandbox_id: record.sandbox_id,
                existing_agent_id: record.agent_id,
                actor: record.input.actor,
            },
        );
        tokio::pin!(work);
        // Claim already granted a fresh lease; renew only after its first period.
        let mut heartbeat =
            tokio::time::interval_at(tokio::time::Instant::now() + HEARTBEAT, HEARTBEAT);
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let outcome = loop {
            tokio::select! {
                biased;
                _ = heartbeat.tick() => self.operations.renew(&lease).await?,
                result = &mut work => break result,
            }
        };
        match outcome {
            Ok(agent_id) => self.operations.succeed(&lease, &agent_id).await,
            Err(AgentLifecycleProvisioningError::Unavailable) => {
                self.operations.retry(&lease, attempts).await
            }
            Err(AgentLifecycleProvisioningError::ConnectionRequired {
                services,
                connection_target,
            }) => {
                self.operations
                    .wait_for_connection(&lease, services, connection_target)
                    .await
            }
            Err(error) => self.operations.fail(&lease, &error.to_string()).await,
        }
    }
}
