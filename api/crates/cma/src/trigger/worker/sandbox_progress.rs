use std::time::SystemTime;

use layer2_core::rfc3339_at;
use layer2_core::sandbox::{
    Sandbox, SandboxOrigin, SandboxStatus, WorkspaceOrigin, WorkspaceStatus,
};
use layer2_core::trigger_domain::{
    Launch, LaunchBlockedReason, LaunchRequirementRefresh, ResolvedResource,
    ServiceRequirementState, WorkspaceReusePolicy,
};

use super::driver::{LaunchResourceEffect, LaunchResourceError};
use super::support::{domain_error, missing_resource, valid_origin_id};
use super::{FrozenLaunchContext, LaunchWorker, LaunchWorkerError};

impl LaunchWorker {
    pub(super) async fn wait_for_sandbox_state(
        &self,
        launch: Launch,
        context: &FrozenLaunchContext,
        now: SystemTime,
    ) -> Result<(), LaunchWorkerError> {
        let (id, reused) = match &launch.resolution.sandbox {
            ResolvedResource::Reused(id) => (id.clone(), true),
            ResolvedResource::Created(id) => (id.clone(), false),
            ResolvedResource::Pending => return self.fail_invalid_sandbox(launch, now).await,
        };
        let Some(sandbox) = self
            .read_sandbox(&launch, &id, reused, context, now)
            .await?
        else {
            return Ok(());
        };
        match sandbox.status {
            SandboxStatus::Active => self.record_sandbox_ready(launch, context, now).await,
            SandboxStatus::Paused => self.revive_waiting_sandbox(launch, id, reused, now).await,
            SandboxStatus::Pausing if sandbox.can_cancel_pause() => {
                self.revive_waiting_sandbox(launch, id, reused, now).await
            }
            SandboxStatus::Reviving => {
                self.block(
                    launch,
                    LaunchBlockedReason::SandboxPausedReviving,
                    None,
                    now,
                )
                .await?;
                Ok(())
            }
            SandboxStatus::Provisioning
            | SandboxStatus::Installing
            | SandboxStatus::Pausing
            | SandboxStatus::CredentialRotating
            | SandboxStatus::Rotating => self.defer(launch, now).await,
            SandboxStatus::Failed if reused => {
                self.create_sandbox(launch, context, true, now).await
            }
            SandboxStatus::Retiring | SandboxStatus::Retired if reused => {
                self.create_sandbox(launch, context, true, now).await
            }
            SandboxStatus::Failed if service_grant_failure(&sandbox).is_some() => {
                self.repair_or_block(launch, &sandbox, now).await
            }
            SandboxStatus::Failed | SandboxStatus::Retiring | SandboxStatus::Retired => {
                self.settle_resource_failure(launch, sandbox_terminal(), now)
                    .await
            }
        }
    }

    async fn repair_or_block(
        &self,
        launch: Launch,
        sandbox: &Sandbox,
        now: SystemTime,
    ) -> Result<(), LaunchWorkerError> {
        let service = service_grant_failure(sandbox).expect("caller checked");
        if launch
            .requirements
            .services
            .iter()
            .filter(|requirement| requirement.service == service)
            .count()
            != 1
        {
            return self
                .settle_resource_failure(launch, invalid_service_failure(), now)
                .await;
        }
        let state = match self
            .resources
            .requirements(&launch.owner, std::slice::from_ref(&service))
            .await
        {
            Ok(requirements) if matches!(requirements.as_slice(), [requirement] if requirement.service == service) => {
                requirements[0].state
            }
            Ok(_) => ServiceRequirementState::Unknown,
            Err(error) => {
                tracing::warn!(launch_id = %launch.id, error = %error, "Sandbox service grant refresh failed");
                ServiceRequirementState::Unknown
            }
        };
        if state == ServiceRequirementState::Satisfied {
            let (caller, _) = self.caller(&launch);
            let id = sandbox.id.clone();
            let launch_id = launch.id.to_string();
            let outcome = self
                .apply_effect(
                    launch,
                    LaunchResourceEffect::RepairSandboxServiceGrant {
                        caller,
                        sandbox_id: id.clone(),
                        launch_id,
                    },
                    now,
                )
                .await?;
            self.after_wait_sandbox_effect(outcome, id, now).await
        } else {
            let refresh = LaunchRequirementRefresh {
                service: service.clone(),
                state,
                checked_at: rfc3339_at(now),
            };
            self.block(
                launch,
                LaunchBlockedReason::ConnectionRequired { service },
                Some(refresh),
                now,
            )
            .await?;
            Ok(())
        }
    }

    async fn record_sandbox_ready(
        &self,
        mut launch: Launch,
        context: &FrozenLaunchContext,
        now: SystemTime,
    ) -> Result<(), LaunchWorkerError> {
        let discovered = if should_discover_workspace(&launch, context) {
            let sandbox_id = launch
                .resolution
                .sandbox
                .id()
                .expect("resolved while waiting");
            match self
                .resources
                .matching_workspace(
                    &launch.owner,
                    sandbox_id,
                    &context.revision.content.target.workspace_profile.id,
                )
                .await
            {
                Ok(Some(workspace)) => {
                    if !valid_discovered_workspace(&workspace, &launch, context) {
                        return self
                            .settle_resource_failure(launch, invalid_discovered_workspace(), now)
                            .await;
                    }
                    Some(workspace.id)
                }
                Ok(None) => None,
                Err(failure) => {
                    return self.settle_resource_failure(launch, failure, now).await;
                }
            }
        } else {
            None
        };
        let expected = launch.revision;
        let fence = self.fence(&launch)?;
        let at = rfc3339_at(now);
        let events = launch
            .record_worker_sandbox_ready(&self.config.holder, fence, discovered, at.clone())
            .map_err(domain_error)?;
        self.commit(expected, launch, at, events).await?;
        Ok(())
    }

    pub(super) async fn read_sandbox(
        &self,
        launch: &Launch,
        id: &layer2_core::SandboxId,
        reused: bool,
        context: &FrozenLaunchContext,
        now: SystemTime,
    ) -> Result<Option<Sandbox>, LaunchWorkerError> {
        let sandbox = match self.resources.sandbox(id).await {
            Ok(Some(sandbox)) => sandbox,
            Ok(None) => {
                self.settle_resource_failure(launch.clone(), missing_resource("sandbox"), now)
                    .await?;
                return Ok(None);
            }
            Err(failure) => {
                self.settle_resource_failure(launch.clone(), failure, now)
                    .await?;
                return Ok(None);
            }
        };
        if !valid_sandbox(&sandbox, launch, context, reused) {
            self.settle_resource_failure(launch.clone(), invalid_sandbox(), now)
                .await?;
            return Ok(None);
        }
        Ok(Some(sandbox))
    }

    pub(super) async fn fail_invalid_sandbox(
        &self,
        launch: Launch,
        now: SystemTime,
    ) -> Result<(), LaunchWorkerError> {
        self.settle_resource_failure(launch, invalid_sandbox(), now)
            .await
    }
}

fn valid_sandbox(
    sandbox: &Sandbox,
    launch: &Launch,
    context: &FrozenLaunchContext,
    reused: bool,
) -> bool {
    let profile = &context.profiles.sandbox;
    let origin_valid = match (&sandbox.origin, reused) {
        (SandboxOrigin::Manual, true) => true,
        (SandboxOrigin::Lifecycle { operation_id }, true) => valid_origin_id(operation_id),
        (SandboxOrigin::Trigger { launch_id }, true) => valid_origin_id(launch_id),
        (SandboxOrigin::Trigger { launch_id }, false) => launch_id == launch.id.as_str(),
        (SandboxOrigin::Manual | SandboxOrigin::Lifecycle { .. }, false) => false,
    };
    launch.resolution.sandbox.id() == Some(&sandbox.id)
        && sandbox.owner == launch.owner
        && sandbox.profile.id == profile.profile_id
        && sandbox.profile.revision == profile.revision
        && sandbox.profile.digest == profile.digest
        && sandbox.profile.content == profile.content
        && origin_valid
}

fn should_discover_workspace(launch: &Launch, context: &FrozenLaunchContext) -> bool {
    launch.resolution.workspace.is_pending()
        && (context.revision.content.reuse.workspace == WorkspaceReusePolicy::ReuseExisting
            || matches!(launch.resolution.sandbox, ResolvedResource::Created(_))
                && context
                    .profiles
                    .sandbox
                    .content
                    .workspace_profiles
                    .contains(&context.revision.content.target.workspace_profile))
}

fn valid_discovered_workspace(
    workspace: &layer2_core::sandbox::Workspace,
    launch: &Launch,
    context: &FrozenLaunchContext,
) -> bool {
    let target = &context.revision.content.target.workspace_profile;
    let profile = &context.profiles.workspace;
    workspace.owner == launch.owner
        && launch.resolution.sandbox.id() == Some(&workspace.sandbox_id)
        && workspace.profile.id == target.id
        && workspace.profile.revision == target.revision
        && workspace.profile.id == profile.profile_id
        && workspace.profile.revision == profile.revision
        && workspace.profile.digest == profile.digest
        && workspace.profile.content == profile.content
        && workspace.status != WorkspaceStatus::Removed
        && valid_workspace_origin(&workspace.origin, context)
}

fn valid_workspace_origin(origin: &WorkspaceOrigin, context: &FrozenLaunchContext) -> bool {
    match origin {
        WorkspaceOrigin::Manual => true,
        WorkspaceOrigin::Trigger { launch_id } => valid_origin_id(launch_id),
        WorkspaceOrigin::SandboxProfile { index } => context
            .profiles
            .sandbox
            .content
            .workspace_profiles
            .get(usize::from(*index))
            .is_some_and(|reference| {
                reference == &context.revision.content.target.workspace_profile
            }),
    }
}

fn service_grant_failure(sandbox: &Sandbox) -> Option<layer2_core::ServiceId> {
    let failure = sandbox.failure.as_ref()?;
    (failure.class == "service_not_granted"
        && !failure.detail.trim().is_empty()
        && failure.detail.len() <= 256
        && !failure.detail.chars().any(char::is_control))
    .then(|| layer2_core::ServiceId::new(&failure.detail))
}

fn invalid_sandbox() -> LaunchResourceError {
    LaunchResourceError::terminal(
        "launch_sandbox_invalid",
        "The Sandbox selected for this launch is not usable",
    )
}

pub(super) fn invalid_sandbox_result() -> LaunchResourceError {
    LaunchResourceError::terminal(
        "sandbox_effect_mismatch",
        "Sandbox recovery returned a different resource",
    )
}

fn invalid_service_failure() -> LaunchResourceError {
    LaunchResourceError::terminal(
        "sandbox_service_failure_invalid",
        "Sandbox service failure does not match this launch",
    )
}

fn invalid_discovered_workspace() -> LaunchResourceError {
    LaunchResourceError::terminal(
        "discovered_workspace_invalid",
        "A discovered Workspace does not match this launch",
    )
}

fn sandbox_terminal() -> LaunchResourceError {
    LaunchResourceError::terminal(
        "sandbox_provisioning_failed",
        "The Sandbox could not be prepared for this launch",
    )
}
