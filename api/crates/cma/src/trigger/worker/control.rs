use std::time::SystemTime;

use layer2_core::rfc3339_at;
use layer2_core::sandbox::SandboxStatus;
use layer2_core::trigger_domain::{
    Launch, LaunchBlockedReason, LaunchRequirementRefresh, LaunchState, ResolvedResource,
    ServiceRequirementState,
};

use super::driver::LaunchResourceError;
use super::support::domain_error;
use super::{LaunchWorker, LaunchWorkerError};

impl LaunchWorker {
    pub(super) async fn prepare_boundary(
        &self,
        launch: Launch,
        now: SystemTime,
    ) -> Result<Option<Launch>, LaunchWorkerError> {
        if launch.state == LaunchState::Ready {
            return Ok(Some(launch));
        }
        let paused = match self.resources.orchestration_paused().await {
            Ok(paused) => paused,
            Err(failure) => {
                self.handle_control_failure(launch, failure, now).await?;
                return Ok(None);
            }
        };
        if paused {
            self.block(launch, LaunchBlockedReason::OrchestrationPaused, None, now)
                .await?;
            return Ok(None);
        }
        match launch.blocked_reason.clone() {
            None => Ok(Some(launch)),
            Some(LaunchBlockedReason::OrchestrationPaused) => self
                .resume(launch, LaunchBlockedReason::OrchestrationPaused, None, now)
                .await
                .map(Some),
            Some(LaunchBlockedReason::ConnectionRequired { service }) => {
                self.refresh_connection_block(launch, service, now).await
            }
            Some(LaunchBlockedReason::SandboxPausedReviving) => {
                self.refresh_reviving_block(launch, now).await
            }
            Some(LaunchBlockedReason::RecoveryRequired) => Err(LaunchWorkerError::Corrupt(
                "non-wakeable recovery Launch was claimed".into(),
            )),
        }
    }

    async fn refresh_connection_block(
        &self,
        launch: Launch,
        service: layer2_core::ServiceId,
        now: SystemTime,
    ) -> Result<Option<Launch>, LaunchWorkerError> {
        let state = match self
            .resources
            .requirements(&launch.owner, std::slice::from_ref(&service))
            .await
        {
            Ok(requirements) if matches!(requirements.as_slice(), [requirement] if requirement.service == service) => {
                requirements[0].state
            }
            Ok(_) => {
                tracing::error!(
                    launch_id = %launch.id,
                    service = %service,
                    "service requirement refresh crossed its requested boundary"
                );
                ServiceRequirementState::Unknown
            }
            Err(error) => {
                tracing::warn!(
                    launch_id = %launch.id,
                    service = %service,
                    error = %error,
                    "service requirement refresh failed"
                );
                ServiceRequirementState::Unknown
            }
        };
        let refresh = Some(LaunchRequirementRefresh {
            service: service.clone(),
            state,
            checked_at: rfc3339_at(now),
        });
        let reason = LaunchBlockedReason::ConnectionRequired { service };
        if state == ServiceRequirementState::Satisfied {
            self.resume(launch, reason, refresh, now).await.map(Some)
        } else {
            self.block(launch, reason, refresh, now).await?;
            Ok(None)
        }
    }

    async fn refresh_reviving_block(
        &self,
        launch: Launch,
        now: SystemTime,
    ) -> Result<Option<Launch>, LaunchWorkerError> {
        let sandbox_id = match &launch.resolution.sandbox {
            ResolvedResource::Reused(id) | ResolvedResource::Created(id) => id,
            ResolvedResource::Pending => {
                return Err(LaunchWorkerError::Corrupt(
                    "reviving Launch has no resolved Sandbox".into(),
                ))
            }
        };
        let status = match self.resources.sandbox(sandbox_id).await {
            Ok(Some(sandbox)) if sandbox.id == *sandbox_id && sandbox.owner == launch.owner => {
                sandbox.status
            }
            Ok(_) => {
                self.block(
                    launch,
                    LaunchBlockedReason::SandboxPausedReviving,
                    None,
                    now,
                )
                .await?;
                return Ok(None);
            }
            Err(error) => {
                tracing::warn!(launch_id = %launch.id, error = %error, "reviving Sandbox read failed");
                self.block(
                    launch,
                    LaunchBlockedReason::SandboxPausedReviving,
                    None,
                    now,
                )
                .await?;
                return Ok(None);
            }
        };
        if matches!(
            status,
            SandboxStatus::Active
                | SandboxStatus::Failed
                | SandboxStatus::Retiring
                | SandboxStatus::Retired
        ) {
            self.resume(
                launch,
                LaunchBlockedReason::SandboxPausedReviving,
                None,
                now,
            )
            .await
            .map(Some)
        } else {
            self.block(
                launch,
                LaunchBlockedReason::SandboxPausedReviving,
                None,
                now,
            )
            .await?;
            Ok(None)
        }
    }

    async fn handle_control_failure(
        &self,
        launch: Launch,
        failure: LaunchResourceError,
        now: SystemTime,
    ) -> Result<(), LaunchWorkerError> {
        let Some(reason) = launch.blocked_reason.clone() else {
            return self.settle_resource_failure(launch, failure, now).await;
        };
        let refresh = match &reason {
            LaunchBlockedReason::ConnectionRequired { service } => Some(LaunchRequirementRefresh {
                service: service.clone(),
                state: ServiceRequirementState::Unknown,
                checked_at: rfc3339_at(now),
            }),
            _ => None,
        };
        tracing::warn!(launch_id = %launch.id, error = %failure, "Launch control refresh failed");
        self.block(launch, reason, refresh, now).await.map(|_| ())
    }

    pub(super) async fn block(
        &self,
        mut launch: Launch,
        reason: LaunchBlockedReason,
        refresh: Option<LaunchRequirementRefresh>,
        now: SystemTime,
    ) -> Result<Launch, LaunchWorkerError> {
        let expected = launch.revision;
        let fence = self.fence(&launch)?;
        let at = rfc3339_at(now);
        let events = launch
            .block_worker_launch(
                &self.config.holder,
                fence,
                reason,
                refresh,
                rfc3339_at(now + self.config.poll_delay),
                at.clone(),
            )
            .map_err(domain_error)?;
        self.commit(expected, launch, at, events).await
    }

    async fn resume(
        &self,
        mut launch: Launch,
        reason: LaunchBlockedReason,
        refresh: Option<LaunchRequirementRefresh>,
        now: SystemTime,
    ) -> Result<Launch, LaunchWorkerError> {
        let expected = launch.revision;
        let fence = self.fence(&launch)?;
        let at = rfc3339_at(now);
        let events = launch
            .resume_worker_launch(&self.config.holder, fence, reason, refresh, at.clone())
            .map_err(domain_error)?;
        self.commit(expected, launch, at, events).await
    }
}
