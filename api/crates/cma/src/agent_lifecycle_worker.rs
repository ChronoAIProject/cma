//! Periodic bounded discovery and cancellable, renewable worker claims.
use super::*;
use crate::store::HEARTBEAT;
use layer1_public::rest::{creation_outcomes::RefusalCode, AgentLifecycleFailure};
use layer2_core::sandbox::store::CreationFence;

pub(super) const PREPARATION_FAILED: &str = "Agent runtime preparation failed";

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
    async fn record_terminal_failure(
        &self,
        owner: &SubjectId,
        lease: &LifecycleLease,
        fence: Option<&CreationFence>,
        message: &str,
        failure: Option<AgentLifecycleFailure>,
        code: RefusalCode,
    ) -> Result<(), layer2_core::StoreError> {
        if let Some(fence) = fence {
            if (code == RefusalCode::SandboxLimitReached
                || self.operations.creation_terminal_settlement_enabled())
                && self
                    .operations
                    .fail_fenced_creation(owner, lease, fence, message, failure.clone(), code)
                    .await?
            {
                return Ok(());
            }
        }
        // A created association is already authoritative. A failure after
        // allocation must update the operation without rewriting that proof.
        self.operations
            .fail_with_metadata(lease, message, failure)
            .await
    }

    pub(crate) fn drain_handle(&self) -> LifecycleDrain {
        self.tasks.clone()
    }

    pub(crate) fn resume_pending(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        if !self.tasks.spawn(async move {
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
        }) {
            tracing::debug!("lifecycle discovery admission closed or locally full");
        }
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
                            PREPARATION_FAILED.into(),
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
        if !self.tasks.spawn(async move {
            let _slot = slot;
            if let Err(error) = this.run_worker(&operation_id).await {
                tracing::error!(operation = %operation_id, error = %error, "Agent lifecycle worker lost storage or claim; discovery will retry");
            }
        }) {
            tracing::debug!("lifecycle worker deferred; durable operation remains discoverable");
        }
    }

    pub(super) async fn run_worker(
        &self,
        operation_id: &str,
    ) -> Result<(), layer2_core::StoreError> {
        let Some(record) = self.operations.claim(operation_id).await? else {
            return Ok(());
        };
        let lease = record.lease()?;
        let creation_fence = record.creation_fence.clone().map(|mut fence| {
            fence.lifecycle_lease = Some((lease.id.clone(), lease.token.clone()));
            fence
        });
        if layer1_public::rest::creation_outcomes::is_covered_key(&record.key)
            != creation_fence.is_some()
        {
            return Err(layer2_core::StoreError::Malformed(
                "lifecycle creation fence is missing or unsupported".into(),
            ));
        }
        if creation_fence
            .as_ref()
            .is_some_and(|fence| !self.operations.supports_creation(fence))
        {
            return Err(layer2_core::StoreError::Unavailable(
                "lifecycle creation epoch is unsupported".into(),
            ));
        }
        let attempts = record.attempts;
        let canary_profiles = match record.verify_canary_worker() {
            Ok(profiles) => profiles,
            Err(error) => {
                return self
                    .record_terminal_failure(
                        &SubjectId::new(record.owner),
                        &lease,
                        creation_fence.as_ref(),
                        &error.to_string(),
                        None,
                        RefusalCode::CreationFailed,
                    )
                    .await;
            }
        };
        let quota_refusals = record.sandbox_quota_refusals;
        let fresh_sandbox = record.input.request.sandbox_id.is_none()
            && record.input.request.workspace_id.is_none()
            && record.sandbox_id.is_none()
            && record.agent_id.is_none()
            && canary_profiles.is_none();
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
        let work = async {
            let mut input = ProvisionInput {
                creation_fence: creation_fence.clone(),
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
                canary_profiles,
                repository_access_pin: record.repository_access_pin,
            };
            self.preflight_repository(
                &caller,
                &mut input,
                record.repository_commit.as_deref(),
                record.public_repository.as_ref(),
            )
            .await?;
            self.provision(&caller, input).await
        };
        tokio::pin!(work);
        // Claim delivery can itself be delayed. Reconfirm the captured token
        // before polling provider work, then bound every renewal wait.
        let mut heartbeat = tokio::time::interval(HEARTBEAT);
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let outcome = loop {
            tokio::select! {
                biased;
                _ = heartbeat.tick() => {
                    tokio::time::timeout(Duration::from_secs(5), self.operations.renew(&lease))
                        .await
                        .map_err(|_| layer2_core::StoreError::Unavailable("lifecycle lease renewal timed out".into()))??;
                },
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
                    .wait_for_connection(&lease, services, connection_target, None)
                    .await
            }
            Err(AgentLifecycleProvisioningError::RepositoryAccessRequired {
                connection_target,
                reason,
                repair,
            }) => {
                self.operations
                    .wait_for_repository_access(
                        &lease,
                        connection_target,
                        reason,
                        repair.map(|detail| *detail),
                    )
                    .await
            }
            Err(error) => {
                // The serialized Sandbox admission can refuse a slot seen at
                // HTTP preflight. Requeue only that confirmed quota refusal.
                if fresh_sandbox
                    && matches!(
                        &error,
                        AgentLifecycleProvisioningError::SandboxLimitReached { .. }
                    )
                    && self
                        .operations
                        .retry_sandbox_quota(&lease, quota_refusals)
                        .await?
                {
                    return Ok(());
                }
                let failure = self
                    .failure_metadata_enabled
                    .get()
                    .copied()
                    .unwrap_or(false)
                    .then_some(match &error {
                        AgentLifecycleProvisioningError::SandboxLimitReached { limit, live } => {
                            Some(
                                layer1_public::rest::AgentLifecycleFailure::SandboxLimitReached {
                                    limit: *limit,
                                    live: *live,
                                },
                            )
                        }
                        _ => None,
                    })
                    .flatten();
                let code = if matches!(
                    error,
                    AgentLifecycleProvisioningError::SandboxLimitReached { .. }
                ) {
                    RefusalCode::SandboxLimitReached
                } else {
                    RefusalCode::CreationFailed
                };
                self.record_terminal_failure(
                    &caller.subject,
                    &lease,
                    creation_fence.as_ref(),
                    &error.to_string(),
                    failure,
                    code,
                )
                .await
            }
        }
    }
}
