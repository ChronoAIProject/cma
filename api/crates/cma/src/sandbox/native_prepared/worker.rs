//! Bounded leased preparation, always saving a transition before the next effect.
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use layer2_core::profiles::{
    AgentProvider, ModelAccess, SandboxProfileContent, WorkspaceLocation, WorkspaceProfileContent,
};
use layer2_core::sandbox::{SandboxStoreError, WarmSandboxState};
use layer2_core::{now_rfc3339, rfc3339_at, ServiceId};
use layer3_adapter::SandboxAdapter;
use tokio::sync::watch;
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::MissedTickBehavior;

use crate::config::Config;
use crate::store::{MongoSandboxStore, NativePreparedClaim, NativePreparedPlan};

pub(crate) struct NativePreparedWorker {
    store: Arc<MongoSandboxStore>,
    adapter: Arc<dyn SandboxAdapter>,
    deployment_id: String,
    desired: Option<NativePreparedPlan>,
    target: u8,
    machine_desired: Option<NativePreparedPlan>,
    machine_target: u8,
}

impl NativePreparedWorker {
    pub(crate) fn new(
        store: Arc<MongoSandboxStore>,
        adapter: Arc<dyn SandboxAdapter>,
        config: &Config,
    ) -> Result<Self, String> {
        // These prototypes pin the home and profile policy. Each refill
        // supplies both images from one captured release.
        let released = store.released_runtime();
        let target = config.runtime.native_prepared_target;
        let machine_target = config.runtime.native_machine_prepared_target;
        let mut desired = if target == 0 && machine_target == 0 {
            None
        } else {
            let services = crate::config::REQUIRED_BASELINE_SERVICES
                .iter()
                .map(|id| ServiceId::new(*id))
                .collect::<Vec<_>>();
            let mut reaches = config.baseline_services.clone();
            reaches.extend(services.clone());
            Some(
                NativePreparedPlan::new(
                    SandboxProfileContent {
                        runtime_image: None,
                        cpu_millis: config.agent_manager_sandbox_capacity.cpu_millis,
                        memory_mib: config.agent_manager_sandbox_capacity.memory_mib,
                        nyxid_services: services,
                        agent_provider: AgentProvider::Codex,
                        model_access: ModelAccess::ChronoaiManaged,
                        max_workspaces: 16,
                        max_agents_per_workspace: 16,
                        workspace_profiles: vec![],
                        software_install_commands: vec![],
                        environment_variables: vec![],
                    },
                    WorkspaceProfileContent {
                        name: "workspace".into(),
                        location: WorkspaceLocation::Directory {
                            name: "workspace".into(),
                        },
                        setup_commands: vec![],
                        agent_profiles: vec![],
                    },
                    crate::agent_lifecycle::prepared_default_agent_profile()?,
                    reaches,
                    released.agent.clone(),
                    config.execution,
                    config
                        .runtime
                        .agent_sandbox_home_policy
                        .clone()
                        .ok_or("native prepared home policy missing")?,
                )
                .map_err(|error| error.to_string())?,
            )
        };
        let machine_desired = if machine_target > 0 {
            let prototype = desired.as_ref().ok_or("prepared prototype missing")?;
            let image = released
                .machine
                .clone()
                .ok_or("prepared machine image missing")?;
            let mut sandbox = prototype.warm.profile.content.clone();
            sandbox.runtime_image = Some(image.clone());
            sandbox.workspace_profiles.clear();
            sandbox.environment_variables = vec![layer2_core::profiles::EnvironmentVariable {
                name: "CMA_MACHINE_INCLUDED".into(),
                value: "1".into(),
            }];
            let mut workspace = prototype.warm.workspaces[0].profile.content.clone();
            workspace.agent_profiles.clear();
            Some(
                NativePreparedPlan::new(
                    sandbox,
                    workspace,
                    prototype.warm.workspaces[0].agents[0].content.clone(),
                    prototype.warm.reaches.clone(),
                    image,
                    config.execution,
                    prototype.home_policy.clone(),
                )
                .map_err(|e| e.to_string())?,
            )
        } else {
            None
        };
        if target == 0 {
            desired = None;
        }
        Ok(Self {
            store,
            adapter,
            deployment_id: config.deployment_id.clone(),
            desired,
            target,
            machine_desired,
            machine_target,
        })
    }

    async fn reconcile_targets(&self) -> Result<(), SandboxStoreError> {
        let release = self.store.released_runtime();
        let agent = self
            .desired
            .as_ref()
            .map(|plan| plan.for_runtime(&release.agent))
            .transpose()?;
        let machine = match (&self.machine_desired, &release.machine) {
            (Some(plan), Some(image)) => Some(plan.for_runtime(image)?),
            (Some(_), None) => return Err(SandboxStoreError::Conflict),
            (None, _) => None,
        };
        let now = now_rfc3339();
        self.store
            .reconcile_native_prepared_release(agent.as_ref(), self.target, &now, false, &release)
            .await?;
        self.store
            .reconcile_native_prepared_release(
                machine.as_ref(),
                self.machine_target,
                &now,
                true,
                &release,
            )
            .await
    }

    pub(crate) fn spawn(self, mut shutdown: watch::Receiver<bool>) -> JoinHandle<()> {
        tokio::spawn(async move {
            let worker = Arc::new(self);
            let mut jobs = JoinSet::new();
            let mut probes = HashMap::new();
            let mut ticker = tokio::time::interval(Duration::from_secs(2));
            ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
            loop {
                if *shutdown.borrow() {
                    break;
                }
                tokio::select! {
                    _ = shutdown.changed() => break,
                    result = jobs.join_next_with_id(), if !jobs.is_empty() => {
                        match result {
                            Some(Ok((id, ()))) => { probes.remove(&id); }
                            Some(Err(error)) => {
                                probes.remove(&error.id());
                                tracing::error!(%error, "native preparation worker task failed");
                            }
                            None => {}
                        }
                    }
                    _ = super::refill::changed(&mut ticker, &worker.store.native_prepared_changed) => {
                        if let Err(error) = worker.reconcile_targets().await {
                            tracing::warn!(%error, "native preparation target reconciliation deferred");
                            continue;
                        }
                    },
                }
                if *shutdown.borrow() {
                    break;
                }
                while jobs.len() < 4 {
                    // Keep the other ready reservation available for admission.
                    // Durable leases continue to fence workers across processes.
                    let ready_classes = [
                        !probes.values().any(|machine: &bool| !*machine),
                        !probes.values().any(|machine| *machine),
                    ];
                    match worker
                        .store
                        .claim_native_prepared_work_for(&now_rfc3339(), &after(60), ready_classes)
                        .await
                    {
                        Ok(Some(claim)) => {
                            let probe = (claim.record.state == WarmSandboxState::Ready)
                                .then_some(claim.plan.machine());
                            let worker = worker.clone();
                            let task = jobs.spawn(async move {
                                if let Err(error) = worker.run_claim(claim).await {
                                    tracing::warn!(%error, "native preparation step requires reconciliation");
                                }
                            });
                            if let Some(machine) = probe {
                                probes.insert(task.id(), machine);
                            }
                        }
                        Ok(None) => break,
                        Err(error) => {
                            tracing::warn!(%error, "native preparation claim deferred");
                            break;
                        }
                    }
                }
            }
            jobs.abort_all();
            while jobs.join_next().await.is_some() {}
        })
    }

    async fn run_claim(&self, claim: NativePreparedClaim) -> Result<(), SandboxStoreError> {
        let operation = async {
            if claim.record.state == WarmSandboxState::Retiring {
                super::retire_step(self.adapter.as_ref(), &claim.record)
                    .await
                    .map(|next| (next, None))
            } else {
                super::prepare_step_with_proof(
                    self.adapter.as_ref(),
                    &self.deployment_id,
                    &claim.record,
                    &claim.plan.home_policy,
                    claim.workspace_proof.clone(),
                )
                .await
            }
        };
        tokio::pin!(operation);
        let mut heartbeat = tokio::time::interval(Duration::from_secs(20));
        heartbeat.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let result = loop {
            tokio::select! {
                biased;
                // The immediate first tick validates the claim before any
                // provider effect. Failed or uncertain renewal cancels work.
                _ = heartbeat.tick() => tokio::time::timeout(
                    Duration::from_secs(5),
                    self.store.renew_native_prepared_work(&claim, &after(60)),
                ).await.map_err(|_| SandboxStoreError::Unavailable(
                    "native preparation lease renewal timed out".into(),
                ))??,
                result = &mut operation => break result,
            }
        };
        let (next, proof, delay) =
            super::complete_step(&claim.record, claim.workspace_proof.as_ref(), result);
        self.store
            .finish_native_prepared_work_with_proof(
                &claim,
                next,
                proof,
                &now_rfc3339(),
                &after(delay),
            )
            .await
    }
}

fn after(seconds: u64) -> String {
    rfc3339_at(SystemTime::now() + Duration::from_secs(seconds))
}
