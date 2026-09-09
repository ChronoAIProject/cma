//! Continuously prepare and replenish unowned environments, without user credentials.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use layer2_core::sandbox::{
    ProvisioningExecution, RuntimeBinding, SandboxPolicySource, SandboxStoreError, WarmSandbox,
    WarmSandboxState,
};
use layer2_core::{now_rfc3339, rfc3339_at};
use layer3_adapter::{InstallRequest, RuntimeAdoptionRequest, RuntimeHandle, SandboxAdapter};
use tokio::sync::watch;
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::MissedTickBehavior;

use super::runtime_mapping::environment_spec;
use crate::store::{MongoSandboxStore, WarmSandboxClaim};

pub(crate) struct WarmSandboxWorker {
    pub home_policy: Option<layer3_adapter::HomeVolumePolicy>,
    pub store: Arc<MongoSandboxStore>,
    pub adapter: Arc<dyn SandboxAdapter>,
    pub policy: Arc<dyn SandboxPolicySource>,
    pub execution: ProvisioningExecution,
    pub deployment_id: String,
    pub runtime_version: String,
}

impl WarmSandboxWorker {
    pub(crate) fn spawn(self, mut shutdown: watch::Receiver<bool>) -> JoinHandle<()> {
        tokio::spawn(async move {
            let worker = Arc::new(self);
            let mut jobs = JoinSet::new();
            let mut ticker = tokio::time::interval(Duration::from_secs(2));
            ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
            loop {
                if *shutdown.borrow() {
                    break;
                }
                tokio::select! {
                    _ = shutdown.changed() => break,
                    result = jobs.join_next(), if !jobs.is_empty() => {
                        if let Some(Err(error)) = result { tracing::error!(%error, "warmup worker task failed"); }
                    }
                    _ = ticker.tick() => (),
                    _ = worker.store.warmup_changed.notified() => (),
                }
                if *shutdown.borrow() {
                    break;
                }
                if let Err(error) = worker.replenish().await {
                    tracing::error!(%error, "warmup pool reconciliation failed");
                    continue;
                }
                while jobs.len() < 10 {
                    let claim = worker
                        .store
                        .claim_warmup_work(&now_rfc3339(), &after(60))
                        .await;
                    match claim {
                        Ok(Some(claim)) => {
                            let worker = worker.clone();
                            jobs.spawn(async move {
                                if let Err(error) = worker.run_claim(claim).await {
                                    tracing::warn!(%error, "warmup step will be reconciled");
                                }
                            });
                        }
                        Ok(None) => break,
                        Err(error) => {
                            tracing::error!(%error, "warmup claim failed");
                            break;
                        }
                    }
                }
            }
            // These jobs cannot own user credentials. Their durable reservations
            // survive shutdown and reclaim the same idempotent operation next boot.
            jobs.abort_all();
            while jobs.join_next().await.is_some() {}
        })
    }

    async fn replenish(&self) -> Result<(), SandboxStoreError> {
        let policy = self
            .policy
            .current_sandbox_policy()
            .await
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
        for id in self.store.warmup_profile_ids().await? {
            self.store
                .reconcile_warmup_target(
                    &id,
                    &self.runtime_version,
                    &policy.baseline_services,
                    self.execution,
                    &now_rfc3339(),
                )
                .await?;
        }
        Ok(())
    }

    async fn run_claim(&self, claim: WarmSandboxClaim) -> Result<(), SandboxStoreError> {
        let operation = prepare_environment(
            self.adapter.as_ref(),
            &self.deployment_id,
            &claim.record,
            self.home_policy.as_ref(),
        );
        tokio::pin!(operation);
        let mut heartbeat = tokio::time::interval(Duration::from_secs(20));
        heartbeat.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let result = loop {
            tokio::select! {
                result = &mut operation => break result,
                _ = heartbeat.tick() => self.store.renew_warmup_work(&claim, &after(60)).await?,
            }
        };
        let (next, delay) = match result {
            Ok(next) => {
                let delay = if next.state == WarmSandboxState::Ready {
                    15
                } else {
                    0
                };
                (next, delay)
            }
            Err(_) => {
                // Provider/install error text may include command output. Only
                // the resource identity and neutral failure are logged here.
                tracing::warn!(sandbox_id = %claim.record.id, "warm environment preparation failed; scheduling replacement");
                let mut next = claim.record.clone();
                next.state = WarmSandboxState::Retiring;
                (next, 30)
            }
        };
        self.store
            .finish_warmup_work(&claim, next, &now_rfc3339(), &after(delay))
            .await
    }
}

pub(super) async fn prepare_environment(
    adapter: &dyn SandboxAdapter,
    deployment_id: &str,
    record: &WarmSandbox,
    home_policy: Option<&layer3_adapter::HomeVolumePolicy>,
) -> Result<WarmSandbox, layer3_adapter::AdapterError> {
    let mut next = record.clone();
    let mut spec = environment_spec(
        &record.id,
        &record.plan.profile,
        &record.plan.reaches,
        deployment_id,
    );
    spec.home_volume = record
        .home
        .as_ref()
        .and_then(|home| home.attachment())
        .map(Box::new);
    match record.state {
        WarmSandboxState::Preparing => {
            if let Some(home) =
                super::home::next_home(adapter, &record.id, record.home.as_deref(), home_policy)
                    .await?
            {
                next.home = Some(Box::new(home));
                return Ok(next);
            }
            let handle = adapter.provision(&spec).await?;
            next.runtime = Some(RuntimeBinding {
                runtime_id: handle.0,
                runtime_version: record.plan.runtime_version.clone(),
                provision_generation: Some(1),
                bound_at: now_rfc3339(),
            });
            next.state = WarmSandboxState::Installing;
        }
        WarmSandboxState::Installing | WarmSandboxState::Ready => {
            let runtime = record.runtime.as_ref().ok_or_else(|| {
                layer3_adapter::AdapterError::Rejected("warm runtime binding missing".into())
            })?;
            let handle = RuntimeHandle::new(&runtime.runtime_id);
            let adoption = RuntimeAdoptionRequest::new(1, handle.clone(), spec)
                .with_expected_image(&record.plan.runtime_version);
            if adapter.adopt_runtime(&adoption).await?.is_none() {
                next.state = WarmSandboxState::Retiring;
                return Ok(next);
            }
            let execution = record.plan.execution;
            let request = InstallRequest::with_timeouts(
                format!("sandbox-install-v1:{}", record.id),
                record
                    .plan
                    .profile
                    .content
                    .software_install_commands
                    .clone(),
                execution.command_timeout_seconds,
                execution.total_timeout_seconds,
                layer3_adapter::DEFAULT_COMMAND_OUTPUT_LIMIT_BYTES,
            );
            let receipt = adapter.run_install(&handle, &request).await?;
            if receipt.operation_id != request.operation_id
                || receipt.request_sha256 != request.request_sha256
                || receipt.commands.len() != request.commands.len()
                || receipt.commands.iter().enumerate().any(|(index, command)| {
                    command.index as usize != index || command.exit_code != 0
                })
            {
                return Err(layer3_adapter::AdapterError::Rejected(
                    "warm installation receipt does not prove completion".into(),
                ));
            }
            next.installed_request_sha256 = Some(receipt.request_sha256);
            next.state = WarmSandboxState::Ready;
        }
        WarmSandboxState::Retiring => {
            for observation in adapter.runtime_inventory().await? {
                if observation.sandbox_id == record.id.as_str() && observation.generation == 1 {
                    adapter.reclaim_runtime(&observation).await?;
                }
            }
            if let Some(home) = &mut next.home {
                super::home::release_home(adapter, home).await?;
            }
            next.state = WarmSandboxState::Retired;
        }
        WarmSandboxState::Claimed | WarmSandboxState::Retired => (),
    }
    Ok(next)
}
fn after(seconds: u64) -> String {
    rfc3339_at(SystemTime::now() + Duration::from_secs(seconds))
}
