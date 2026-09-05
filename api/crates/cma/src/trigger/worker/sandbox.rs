use std::time::SystemTime;

use layer2_core::rfc3339_at;
use layer2_core::sandbox::{CreateSandbox, SandboxOrigin, SandboxStatus};
use layer2_core::trigger_domain::{Launch, LaunchEffectKind, ResolvedResource};

use super::driver::LaunchResourceEffect;
use super::effects::EffectOutcome;
use super::sandbox_progress::invalid_sandbox_result;
use super::support::{domain_error, validate_sandbox_id};
use super::{FrozenLaunchContext, LaunchWorker, LaunchWorkerError};

impl LaunchWorker {
    pub(super) async fn resolve_sandbox(
        &self,
        launch: Launch,
        context: &FrozenLaunchContext,
        now: SystemTime,
    ) -> Result<(), LaunchWorkerError> {
        if let Some(effect) = launch.work.effect.as_ref() {
            return match effect.kind {
                LaunchEffectKind::CreateSandbox => {
                    self.create_sandbox(launch, context, false, now).await
                }
                LaunchEffectKind::ReviveSandbox => self.revive_resolved_sandbox(launch, now).await,
                _ => self.fail_invalid_sandbox(launch, now).await,
            };
        }
        match &launch.resolution.sandbox {
            ResolvedResource::Pending => self.create_sandbox(launch, context, false, now).await,
            ResolvedResource::Reused(id) => {
                let Some(sandbox) = self.read_sandbox(&launch, id, true, context, now).await?
                else {
                    return Ok(());
                };
                if sandbox.status == SandboxStatus::Paused {
                    self.revive_resolved_sandbox(launch, now).await
                } else {
                    self.record_sandbox(launch, sandbox.id, true, now).await
                }
            }
            ResolvedResource::Created(_) => self.fail_invalid_sandbox(launch, now).await,
        }
    }

    pub(super) async fn wait_for_sandbox(
        &self,
        launch: Launch,
        context: &FrozenLaunchContext,
        now: SystemTime,
    ) -> Result<(), LaunchWorkerError> {
        if let Some(effect) = launch.work.effect.as_ref() {
            return match effect.kind {
                LaunchEffectKind::CreateSandbox => {
                    self.create_sandbox(launch, context, true, now).await
                }
                LaunchEffectKind::ReviveSandbox => {
                    self.apply_wait_sandbox_effect(launch, true, now).await
                }
                LaunchEffectKind::RepairSandboxServiceGrant => {
                    self.apply_wait_sandbox_effect(launch, false, now).await
                }
                _ => self.fail_invalid_sandbox(launch, now).await,
            };
        }
        self.wait_for_sandbox_state(launch, context, now).await
    }

    pub(super) async fn create_sandbox(
        &self,
        launch: Launch,
        context: &FrozenLaunchContext,
        replacing: bool,
        now: SystemTime,
    ) -> Result<(), LaunchWorkerError> {
        let previous = if replacing {
            match &launch.resolution.sandbox {
                ResolvedResource::Reused(id) => Some(id.clone()),
                _ => return self.fail_invalid_sandbox(launch, now).await,
            }
        } else if launch.resolution.sandbox.is_pending() {
            None
        } else {
            return self.fail_invalid_sandbox(launch, now).await;
        };
        let (caller, authority) = self.caller(&launch);
        let request = CreateSandbox {
            profile: context.revision.content.target.sandbox_profile.clone(),
            name: None,
            confirmed_digest: launch.trust.sandbox_digest.clone(),
            origin: SandboxOrigin::Trigger {
                launch_id: launch.id.to_string(),
            },
        };
        let outcome = self
            .apply_effect(
                launch,
                LaunchResourceEffect::CreateSandbox {
                    caller,
                    authority,
                    request,
                },
                now,
            )
            .await?;
        let EffectOutcome::Applied { launch, result_id } = outcome else {
            return Ok(());
        };
        let mut launch = *launch;
        let sandbox_id = match validate_sandbox_id(&result_id) {
            Ok(id) => id,
            Err(failure) => return self.settle_resource_failure(launch, failure, now).await,
        };
        let expected = launch.revision;
        let fence = self.fence(&launch)?;
        let at = rfc3339_at(now);
        let events = if let Some(previous) = previous {
            launch.replace_worker_reused_sandbox(
                &self.config.holder,
                fence,
                &previous,
                sandbox_id,
                at.clone(),
            )
        } else {
            launch.record_worker_sandbox(&self.config.holder, fence, sandbox_id, false, at.clone())
        }
        .map_err(domain_error)?;
        self.commit(expected, launch, at, events).await?;
        Ok(())
    }

    async fn revive_resolved_sandbox(
        &self,
        launch: Launch,
        now: SystemTime,
    ) -> Result<(), LaunchWorkerError> {
        let id = match &launch.resolution.sandbox {
            ResolvedResource::Reused(id) => id.clone(),
            _ => return self.fail_invalid_sandbox(launch, now).await,
        };
        let (caller, _) = self.caller(&launch);
        let launch_id = launch.id.to_string();
        let outcome = self
            .apply_effect(
                launch,
                LaunchResourceEffect::ReviveSandbox {
                    caller,
                    sandbox_id: id.clone(),
                    launch_id,
                    reused: true,
                },
                now,
            )
            .await?;
        let EffectOutcome::Applied { launch, result_id } = outcome else {
            return Ok(());
        };
        let launch = *launch;
        if result_id != id.as_str() {
            return self
                .settle_resource_failure(launch, invalid_sandbox_result(), now)
                .await;
        }
        self.record_sandbox(launch, id, true, now).await
    }

    pub(super) async fn revive_waiting_sandbox(
        &self,
        launch: Launch,
        id: layer2_core::SandboxId,
        reused: bool,
        now: SystemTime,
    ) -> Result<(), LaunchWorkerError> {
        let (caller, _) = self.caller(&launch);
        let launch_id = launch.id.to_string();
        let outcome = self
            .apply_effect(
                launch,
                LaunchResourceEffect::ReviveSandbox {
                    caller,
                    sandbox_id: id.clone(),
                    launch_id,
                    reused,
                },
                now,
            )
            .await?;
        self.after_wait_sandbox_effect(outcome, id, now).await
    }

    async fn apply_wait_sandbox_effect(
        &self,
        launch: Launch,
        revive: bool,
        now: SystemTime,
    ) -> Result<(), LaunchWorkerError> {
        let (id, reused) = resolved_sandbox(&launch)?;
        let (caller, _) = self.caller(&launch);
        let launch_id = launch.id.to_string();
        let request = if revive {
            LaunchResourceEffect::ReviveSandbox {
                caller,
                sandbox_id: id.clone(),
                launch_id,
                reused,
            }
        } else {
            LaunchResourceEffect::RepairSandboxServiceGrant {
                caller,
                sandbox_id: id.clone(),
                launch_id,
            }
        };
        let outcome = self.apply_effect(launch, request, now).await?;
        self.after_wait_sandbox_effect(outcome, id, now).await
    }

    pub(super) async fn after_wait_sandbox_effect(
        &self,
        outcome: EffectOutcome,
        expected_id: layer2_core::SandboxId,
        now: SystemTime,
    ) -> Result<(), LaunchWorkerError> {
        let EffectOutcome::Applied { launch, result_id } = outcome else {
            return Ok(());
        };
        let mut launch = *launch;
        if result_id != expected_id.as_str() {
            return self
                .settle_resource_failure(launch, invalid_sandbox_result(), now)
                .await;
        }
        let expected = launch.revision;
        let fence = self.fence(&launch)?;
        let at = rfc3339_at(now);
        let events = launch
            .settle_worker_sandbox_effect(
                &self.config.holder,
                fence,
                &expected_id,
                rfc3339_at(now + self.config.poll_delay),
                at.clone(),
            )
            .map_err(domain_error)?;
        self.commit(expected, launch, at, events).await?;
        Ok(())
    }

    async fn record_sandbox(
        &self,
        mut launch: Launch,
        sandbox_id: layer2_core::SandboxId,
        reused: bool,
        now: SystemTime,
    ) -> Result<(), LaunchWorkerError> {
        let expected = launch.revision;
        let fence = self.fence(&launch)?;
        let at = rfc3339_at(now);
        let events = launch
            .record_worker_sandbox(&self.config.holder, fence, sandbox_id, reused, at.clone())
            .map_err(domain_error)?;
        self.commit(expected, launch, at, events).await?;
        Ok(())
    }
}

fn resolved_sandbox(launch: &Launch) -> Result<(layer2_core::SandboxId, bool), LaunchWorkerError> {
    match &launch.resolution.sandbox {
        ResolvedResource::Reused(id) => Ok((id.clone(), true)),
        ResolvedResource::Created(id) => Ok((id.clone(), false)),
        ResolvedResource::Pending => Err(LaunchWorkerError::Corrupt(
            "Sandbox effect has no resolved target".into(),
        )),
    }
}
