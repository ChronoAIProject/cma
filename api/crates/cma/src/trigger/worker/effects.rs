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
                self.settle_resource_failure(launch, failure, now).await?;
                return Ok(EffectOutcome::Halted);
            }
        };
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
