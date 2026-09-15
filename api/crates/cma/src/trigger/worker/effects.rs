use std::time::SystemTime;

use layer2_core::rfc3339_at;
use layer2_core::trigger_domain::{Launch, LaunchEffectKind, LaunchEffectState};

use super::driver::{LaunchResourceEffect, LaunchResourceError};
use super::support::{domain_error, valid_resource_id};
use super::{LaunchWorker, LaunchWorkerError};

pub(super) enum EffectOutcome {
    Applied {
        launch: Box<Launch>,
        result_id: String,
    },
    Halted,
}

impl LaunchWorker {
    pub(super) async fn apply_effect(
        &self,
        mut launch: Launch,
        request: LaunchResourceEffect,
        now: SystemTime,
    ) -> Result<EffectOutcome, LaunchWorkerError> {
        let kind = request.kind();
        if launch
            .work
            .effect
            .as_ref()
            .is_some_and(|effect| effect.kind != kind)
        {
            self.settle_resource_failure(launch, invalid_effect(), now)
                .await?;
            return Ok(EffectOutcome::Halted);
        }
        match self.checkpoint_blocks_effect(&launch, kind).await {
            Ok(Some(sandbox)) => {
                self.defer_for_checkpoint(launch, &sandbox, now).await?;
                return Ok(EffectOutcome::Halted);
            }
            Ok(None) => {}
            Err(failure) => {
                self.settle_resource_failure(launch, failure, now).await?;
                return Ok(EffectOutcome::Halted);
            }
        }
        if launch.work.effect.is_none() {
            launch = self.prepare_effect(launch, kind, now).await?;
        }
        if launch
            .work
            .effect
            .as_ref()
            .is_some_and(|effect| effect.state == LaunchEffectState::Applied)
        {
            let result_id = launch
                .work
                .effect
                .as_ref()
                .and_then(|effect| effect.result_id.clone())
                .ok_or_else(|| LaunchWorkerError::Corrupt("applied effect has no result".into()))?;
            return Ok(EffectOutcome::Applied {
                launch: Box::new(launch),
                result_id,
            });
        }
        if launch
            .work
            .effect
            .as_ref()
            .is_some_and(|effect| effect.state == LaunchEffectState::Prepared)
        {
            launch = self.mark_effect_executing(launch, now).await?;
        }
        let state = launch
            .work
            .effect
            .as_ref()
            .map(|effect| effect.state)
            .ok_or_else(|| LaunchWorkerError::Corrupt("launch effect disappeared".into()))?;
        if !matches!(
            state,
            LaunchEffectState::Executing | LaunchEffectState::RecoveryRequired
        ) {
            self.settle_resource_failure(launch, invalid_effect(), now)
                .await?;
            return Ok(EffectOutcome::Halted);
        }

        let result_id = match self.resources.apply(request).await {
            Ok(result_id) if valid_effect_result(kind, &result_id) => result_id,
            Ok(_) => {
                self.settle_resource_failure(launch, invalid_effect_result(), now)
                    .await?;
                return Ok(EffectOutcome::Halted);
            }
            Err(failure) => {
                if failure.code == "launch_sandbox_not_ready" {
                    if let Ok(Some(sandbox)) = self.checkpoint_blocks_effect(&launch, kind).await {
                        self.defer_for_checkpoint(launch, &sandbox, now).await?;
                        return Ok(EffectOutcome::Halted);
                    }
                }
                self.settle_resource_failure(launch, failure, now).await?;
                return Ok(EffectOutcome::Halted);
            }
        };
        if kind == LaunchEffectKind::CreateAgent && launch.input_protocol.is_some() {
            let durable = self
                .launches
                .launch(&launch.id, &launch.owner)
                .await?
                .ok_or_else(|| LaunchWorkerError::Corrupt("accepted Launch disappeared".into()))?;
            let mut expected = launch.clone();
            if expected.input_binding_sha256.is_none() && durable.input_binding_sha256.is_some() {
                expected.input_binding_sha256 = durable.input_binding_sha256.clone();
                expected.first_message_command = durable.first_message_command.clone();
                expected.revision = expected
                    .revision
                    .checked_add(1)
                    .ok_or_else(|| LaunchWorkerError::Corrupt("Launch revision overflow".into()))?;
            }
            if durable != expected {
                return Err(layer2_core::trigger_domain::LaunchStoreError::Conflict {
                    expected: launch.revision,
                    actual: durable.revision,
                }
                .into());
            }
            launch = durable;
        }
        let expected = launch.revision;
        let fence = self.fence(&launch)?;
        let at = rfc3339_at(now);
        let events = match state {
            LaunchEffectState::Executing => {
                launch
                    .mark_worker_effect_applied(
                        &self.config.holder,
                        fence,
                        result_id.clone(),
                        at.clone(),
                    )
                    .map_err(domain_error)?;
                Vec::new()
            }
            LaunchEffectState::RecoveryRequired => launch
                .reconcile_worker_effect(
                    &self.config.holder,
                    fence,
                    kind,
                    result_id.clone(),
                    at.clone(),
                )
                .map_err(domain_error)?,
            LaunchEffectState::Prepared | LaunchEffectState::Applied => unreachable!("checked"),
        };
        let launch = self.commit(expected, launch, at, events).await?;
        Ok(EffectOutcome::Applied {
            launch: Box::new(launch),
            result_id,
        })
    }

    async fn checkpoint_blocks_effect(
        &self,
        launch: &Launch,
        kind: LaunchEffectKind,
    ) -> Result<Option<layer2_core::sandbox::Sandbox>, LaunchResourceError> {
        if !matches!(
            kind,
            LaunchEffectKind::CreateWorkspace | LaunchEffectKind::CreateAgent
        ) || launch
            .work
            .effect
            .as_ref()
            .is_some_and(|effect| effect.state == LaunchEffectState::Applied)
        {
            return Ok(None);
        }
        let Some(id) = launch.resolution.sandbox.id() else {
            return Err(invalid_effect());
        };
        let sandbox = self.resources.sandbox(id).await?;
        Ok(sandbox.filter(|sandbox| {
            sandbox.id == *id
                && sandbox.owner == launch.owner
                && sandbox.status == layer2_core::sandbox::SandboxStatus::Active
                && sandbox.lifecycle.checkpoint_capture.is_some()
        }))
    }

    async fn defer_for_checkpoint(
        &self,
        mut launch: Launch,
        sandbox: &layer2_core::sandbox::Sandbox,
        now: SystemTime,
    ) -> Result<(), LaunchWorkerError> {
        let expected = launch.revision;
        let fence = self.fence(&launch)?;
        let at = rfc3339_at(now);
        let events = launch
            .defer_worker_for_checkpoint(
                sandbox,
                &self.config.holder,
                fence,
                rfc3339_at(now + self.config.poll_delay),
                at.clone(),
            )
            .map_err(domain_error)?;
        self.commit(expected, launch, at, events).await?;
        Ok(())
    }

    async fn prepare_effect(
        &self,
        mut launch: Launch,
        kind: LaunchEffectKind,
        now: SystemTime,
    ) -> Result<Launch, LaunchWorkerError> {
        let expected = launch.revision;
        let fence = self.fence(&launch)?;
        let at = rfc3339_at(now);
        let events = launch
            .prepare_worker_effect(&self.config.holder, fence, kind, at.clone())
            .map_err(domain_error)?;
        self.commit(expected, launch, at, events).await
    }

    async fn mark_effect_executing(
        &self,
        mut launch: Launch,
        now: SystemTime,
    ) -> Result<Launch, LaunchWorkerError> {
        let expected = launch.revision;
        let fence = self.fence(&launch)?;
        let at = rfc3339_at(now);
        launch
            .mark_worker_effect_executing(&self.config.holder, fence, at.clone())
            .map_err(domain_error)?;
        self.commit(expected, launch, at, Vec::new()).await
    }
}

fn valid_effect_result(kind: LaunchEffectKind, value: &str) -> bool {
    let prefix = match kind {
        LaunchEffectKind::CreateSandbox
        | LaunchEffectKind::ReviveSandbox
        | LaunchEffectKind::RepairSandboxServiceGrant => "sbx_",
        LaunchEffectKind::CreateWorkspace => "wks_",
        LaunchEffectKind::CreateAgent => "agt_",
    };
    valid_resource_id(value, prefix)
}

fn invalid_effect() -> LaunchResourceError {
    LaunchResourceError::terminal(
        "launch_effect_invalid",
        "Durable launch effect data is not usable",
    )
}

fn invalid_effect_result() -> LaunchResourceError {
    LaunchResourceError::terminal(
        "launch_effect_result_invalid",
        "A launch dependency returned invalid resource evidence",
    )
}
