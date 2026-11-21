use std::time::SystemTime;

use layer2_core::rfc3339_at;
use layer2_core::sandbox::{CreateWorkspace, Workspace, WorkspaceOrigin, WorkspaceStatus};
use layer2_core::trigger_domain::{Launch, LaunchEffectKind, ResolvedResource};

use super::driver::{LaunchResourceEffect, LaunchResourceError};
use super::effects::EffectOutcome;
use super::support::{domain_error, missing_resource, valid_origin_id, validate_workspace_id};
use super::{FrozenLaunchContext, LaunchWorker, LaunchWorkerError};

impl LaunchWorker {
    pub(super) async fn resolve_workspace(
        &self,
        launch: Launch,
        context: &FrozenLaunchContext,
        now: SystemTime,
    ) -> Result<(), LaunchWorkerError> {
        if let Some(effect) = launch.work.effect.as_ref() {
            return if effect.kind == LaunchEffectKind::CreateWorkspace {
                self.create_workspace(launch, context, false, now).await
            } else {
                self.fail_invalid_workspace(launch, now).await
            };
        }
        match &launch.resolution.workspace {
            ResolvedResource::Pending => self.create_workspace(launch, context, false, now).await,
            ResolvedResource::Reused(id) => {
                let Some(workspace) = self.read_workspace(&launch, id, true, context, now).await?
                else {
                    return Ok(());
                };
                self.record_workspace(launch, workspace.id, true, now).await
            }
            ResolvedResource::Created(_) => self.fail_invalid_workspace(launch, now).await,
        }
    }

    pub(super) async fn wait_for_workspace(
        &self,
        launch: Launch,
        context: &FrozenLaunchContext,
        now: SystemTime,
    ) -> Result<(), LaunchWorkerError> {
        if let Some(effect) = launch.work.effect.as_ref() {
            return if effect.kind == LaunchEffectKind::CreateWorkspace {
                self.create_workspace(launch, context, true, now).await
            } else {
                self.fail_invalid_workspace(launch, now).await
            };
        }
        let (id, reused) = match &launch.resolution.workspace {
            ResolvedResource::Reused(id) => (id.clone(), true),
            ResolvedResource::Created(id) => (id.clone(), false),
            ResolvedResource::Pending => return self.fail_invalid_workspace(launch, now).await,
        };
        let Some(workspace) = self
            .read_workspace(&launch, &id, reused, context, now)
            .await?
        else {
            return Ok(());
        };
        match workspace.status {
            WorkspaceStatus::Ready => self.record_workspace_ready(launch, now).await,
            WorkspaceStatus::Preparing => self.defer(launch, now).await,
            WorkspaceStatus::Failed | WorkspaceStatus::Removing | WorkspaceStatus::Removed
                if reused =>
            {
                self.create_workspace(launch, context, true, now).await
            }
            WorkspaceStatus::Failed | WorkspaceStatus::Removing | WorkspaceStatus::Removed => {
                self.settle_resource_failure(launch, workspace_terminal(), now)
                    .await
            }
        }
    }

    async fn create_workspace(
        &self,
        launch: Launch,
        context: &FrozenLaunchContext,
        replacing: bool,
        now: SystemTime,
    ) -> Result<(), LaunchWorkerError> {
        let previous = if replacing {
            match &launch.resolution.workspace {
                ResolvedResource::Reused(id) => Some(id.clone()),
                _ => return self.fail_invalid_workspace(launch, now).await,
            }
        } else if launch.resolution.workspace.is_pending() {
            None
        } else {
            return self.fail_invalid_workspace(launch, now).await;
        };
        let Some(sandbox_id) = launch.resolution.sandbox.id().cloned() else {
            return self.fail_invalid_workspace(launch, now).await;
        };
        let (caller, authority) = self.caller(&launch);
        let launch_id = launch.id.to_string();
        let request = CreateWorkspace {
            profile: context.revision.content.target.workspace_profile.clone(),
            confirmed_digest: launch.trust.workspace_digest.clone(),
        };
        let outcome = self
            .apply_effect(
                launch,
                LaunchResourceEffect::CreateWorkspace {
                    caller,
                    authority,
                    sandbox_id,
                    request,
                    launch_id,
                },
                now,
            )
            .await?;
        let EffectOutcome::Applied { launch, result_id } = outcome else {
            return Ok(());
        };
        let mut launch = *launch;
        let workspace_id = match validate_workspace_id(&result_id) {
            Ok(id) => id,
            Err(failure) => return self.settle_resource_failure(launch, failure, now).await,
        };
        let expected = launch.revision;
        let fence = self.fence(&launch)?;
        let at = rfc3339_at(now);
        let events = if let Some(previous) = previous {
            launch.replace_worker_reused_workspace(
                &self.config.holder,
                fence,
                &previous,
                workspace_id,
                at.clone(),
            )
        } else {
            launch.record_worker_workspace(
                &self.config.holder,
                fence,
                workspace_id,
                false,
                at.clone(),
            )
        }
        .map_err(domain_error)?;
        self.commit(expected, launch, at, events).await?;
        Ok(())
    }

    async fn record_workspace(
        &self,
        mut launch: Launch,
        workspace_id: layer2_core::WorkspaceId,
        reused: bool,
        now: SystemTime,
    ) -> Result<(), LaunchWorkerError> {
        let expected = launch.revision;
        let fence = self.fence(&launch)?;
        let at = rfc3339_at(now);
        let events = launch
            .record_worker_workspace(&self.config.holder, fence, workspace_id, reused, at.clone())
            .map_err(domain_error)?;
        self.commit(expected, launch, at, events).await?;
        Ok(())
    }

    async fn record_workspace_ready(
        &self,
        mut launch: Launch,
        now: SystemTime,
    ) -> Result<(), LaunchWorkerError> {
        let expected = launch.revision;
        let fence = self.fence(&launch)?;
        let at = rfc3339_at(now);
        let events = launch
            .record_worker_workspace_ready(&self.config.holder, fence, at.clone())
            .map_err(domain_error)?;
        self.commit(expected, launch, at, events).await?;
        Ok(())
    }

    async fn read_workspace(
        &self,
        launch: &Launch,
        id: &layer2_core::WorkspaceId,
        reused: bool,
        context: &FrozenLaunchContext,
        now: SystemTime,
    ) -> Result<Option<Workspace>, LaunchWorkerError> {
        let workspace = match self.resources.workspace(id).await {
            Ok(Some(workspace)) => workspace,
            Ok(None) => {
                self.settle_resource_failure(launch.clone(), missing_resource("workspace"), now)
                    .await?;
                return Ok(None);
            }
            Err(failure) => {
                self.settle_resource_failure(launch.clone(), failure, now)
                    .await?;
                return Ok(None);
            }
        };
        if !valid_workspace(&workspace, launch, context, reused) {
            self.settle_resource_failure(launch.clone(), invalid_workspace(), now)
                .await?;
            return Ok(None);
        }
        Ok(Some(workspace))
    }

    async fn fail_invalid_workspace(
        &self,
        launch: Launch,
        now: SystemTime,
    ) -> Result<(), LaunchWorkerError> {
        self.settle_resource_failure(launch, invalid_workspace(), now)
            .await
    }
}

fn valid_workspace(
    workspace: &Workspace,
    launch: &Launch,
    context: &FrozenLaunchContext,
    reused: bool,
) -> bool {
    let profile = &context.profiles.workspace;
    let origin_valid = match (&workspace.origin, reused) {
        (WorkspaceOrigin::Manual, true) => true,
        (WorkspaceOrigin::SandboxProfile { index }, true) => context
            .profiles
            .sandbox
            .content
            .workspace_profiles
            .get(usize::from(*index))
            .is_some_and(|reference| {
                reference.id == profile.profile_id && reference.revision == profile.revision
            }),
        (WorkspaceOrigin::Trigger { launch_id }, true) => valid_origin_id(launch_id),
        (WorkspaceOrigin::Lifecycle { operation_id }, true) => valid_origin_id(operation_id),
        (WorkspaceOrigin::Lifecycle { .. }, false) => false,
        (WorkspaceOrigin::Trigger { launch_id }, false) => launch_id == launch.id.as_str(),
        (WorkspaceOrigin::Manual | WorkspaceOrigin::SandboxProfile { .. }, false) => false,
    };
    launch.resolution.workspace.id() == Some(&workspace.id)
        && launch.resolution.sandbox.id() == Some(&workspace.sandbox_id)
        && workspace.owner == launch.owner
        && workspace.profile.id == profile.profile_id
        && workspace.profile.revision == profile.revision
        && workspace.profile.digest == profile.digest
        && workspace.profile.content == profile.content
        && origin_valid
}

fn invalid_workspace() -> LaunchResourceError {
    LaunchResourceError::terminal(
        "launch_workspace_invalid",
        "The Workspace selected for this launch is not usable",
    )
}

fn workspace_terminal() -> LaunchResourceError {
    LaunchResourceError::terminal(
        "workspace_provisioning_failed",
        "The Workspace could not be prepared for this launch",
    )
}
